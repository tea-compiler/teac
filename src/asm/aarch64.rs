//! AArch64 backend driver: lowers an IR module to instructions over
//! virtual registers, register-allocates each function, and delegates
//! textual emission of the final assembly to [`printer::AsmPrinter`].

mod aapcs;
mod frame;
mod function_generator;
mod inst;
mod phi_lowering;
mod printer;
mod register_allocator;
mod types;

use crate::asm::common::StructLayouts;
use crate::asm::error::Error;
use crate::common::{Generator, Target};
use crate::ir;
use aapcs::{classify_args, ArgumentLocation};
use frame::FrameLayout;
use function_generator::AsmFunctionGenerator;
use inst::Instruction;
use printer::AsmPrinter;
use register_allocator::RegisterAllocator;
use std::io::Write;
use types::{InstOperand, Register, RegisterSize};

struct GeneratedGlobal {
    symbol: String,
    data: GlobalData,
}

enum GlobalData {
    Word { value: i64 },
    Array { words: Vec<i64>, zero_bytes: i64 },
}

struct GeneratedFunction {
    symbol: String,
    frame_size: i64,
    insts: Vec<Instruction>,
    /// Whether this function uses any FP register.  Drives the FP half
    /// of the caller-saved bracket: only FP-using functions preserve
    /// the `s18`–`s25` pool around their calls.
    uses_fp: bool,
}

/// Whether the instruction stream uses any FP register — a dedicated
/// FP opcode or an `S32`-sized `Mov` / `Ldr` / `Str`.  Drives the FP
/// half of the caller-saved register bracket.
fn stream_uses_fp(insts: &[Instruction]) -> bool {
    insts.iter().any(|i| {
        matches!(
            i,
            Instruction::FBinOp { .. }
                | Instruction::FCmp { .. }
                | Instruction::Scvtf { .. }
                | Instruction::Fcvtzs { .. }
                | Instruction::Fmov { .. }
                | Instruction::Mov {
                    size: RegisterSize::S32,
                    ..
                }
                | Instruction::Ldr {
                    size: RegisterSize::S32,
                    ..
                }
                | Instruction::Str {
                    size: RegisterSize::S32,
                    ..
                }
        )
    })
}

/// AArch64 assembly generator: runs instruction selection and register
/// allocation for every function of the module, together with data
/// layout for globals; the `Generator` impl drives both phases.
pub struct AArch64AsmGenerator<'a> {
    module: &'a ir::Module,
    registry: &'a ir::Registry,
    target: Target,
    globals: Vec<GeneratedGlobal>,
    functions: Vec<GeneratedFunction>,
}

impl<'a> AArch64AsmGenerator<'a> {
    pub fn new(module: &'a ir::Module, registry: &'a ir::Registry, target: Target) -> Self {
        Self {
            module,
            registry,
            target,
            globals: Vec::new(),
            functions: Vec::new(),
        }
    }
}

impl<'a> Generator for AArch64AsmGenerator<'a> {
    type Error = Error;

    fn generate(&mut self) -> Result<(), Error> {
        let layouts = StructLayouts::from_struct_types(&self.registry.struct_types)?;

        self.globals.clear();
        for (name, def) in &self.module.global_list {
            self.globals
                .push(Self::handle_global(&layouts, name, def, self.target)?);
        }

        self.functions.clear();
        for func in self.module.function_list.values() {
            // Skip external declarations; they are provided by the linked
            // object file (e.g. std.o) and must not be emitted as assembly
            // symbols, otherwise the linker will report duplicate definitions.
            let Some(body) = func.body.as_ref() else {
                continue;
            };
            self.functions.push(Self::handle_function(
                &layouts,
                &func.link_name,
                body,
                self.target,
            )?);
        }

        Ok(())
    }

    /// Delegates to [`AsmPrinter::emit_program`], mirroring how the ir
    /// layer's `output` delegates to `IrPrinter::emit_module` — the
    /// printer owns section layout, symbol emission, and prologues.
    fn output<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        AsmPrinter::new(w, self.target).emit_program(&self.globals, &self.functions)
    }
}

impl<'a> AArch64AsmGenerator<'a> {
    /// Emits the function-entry shim that lifts each incoming AAPCS64
    /// argument from its calling-convention slot (`x_`, `s_`, or
    /// stack) into the vreg that the function body refers to.
    /// Classification is delegated to [`classify_args`] so that the
    /// callee and the call-site shim in `function_generator::emit_call`
    /// agree on every argument's location by construction.
    fn handle_arguments(body: &ir::FunctionBody) -> Result<Vec<Instruction>, Error> {
        let locs = classify_args(body.arguments.iter().map(|a| &a.dtype))?;
        let mut insts = Vec::with_capacity(body.arguments.len());

        for (arg, loc) in body.arguments.iter().zip(locs) {
            let dst = Register::Virtual(arg.id.0);
            let size = RegisterSize::try_from(&arg.dtype)?;

            let inst = match loc {
                ArgumentLocation::Gpr(n) => Instruction::Mov {
                    size,
                    dst,
                    src: InstOperand::Register(Register::Physical(n)),
                },
                ArgumentLocation::Fpr(_) => todo!(
                    "asmt-4: lift an incoming f32 argument out of its `s_` register \
                     into `dst` via fmov; see asmt-4.md §3.3"
                ),
                ArgumentLocation::Stack { offset } => Instruction::Ldr {
                    size,
                    dst,
                    addr: FrameLayout::incoming_arg_addr(offset),
                },
            };
            insts.push(inst);
        }

        Ok(insts)
    }

    fn handle_global(
        layouts: &StructLayouts,
        name: &str,
        def: &ir::GlobalDef,
        target: Target,
    ) -> Result<GeneratedGlobal, Error> {
        let symbol = target.mangle_symbol(name);

        let data = match &def.dtype {
            ir::Dtype::I32 => {
                let value = def
                    .initializers
                    .as_ref()
                    .and_then(|v| v.first())
                    .copied()
                    .map(|v| v as i64)
                    .unwrap_or(0);
                GlobalData::Word { value }
            }
            ir::Dtype::Array { element, length } => {
                let len = length.expect("unsized array in global data");
                let (elem_size, _) = layouts.size_align_of(element.as_ref())?;

                if let Some(inits) = &def.initializers {
                    let words: Vec<i64> = inits.iter().take(len).map(|&v| v as i64).collect();
                    let remaining = len.saturating_sub(inits.len());
                    let zero_bytes = (remaining as i64) * elem_size;
                    GlobalData::Array { words, zero_bytes }
                } else {
                    let zero_bytes = (len as i64) * elem_size;
                    GlobalData::Array {
                        words: Vec::new(),
                        zero_bytes,
                    }
                }
            }
            _ => {
                return Err(Error::UnsupportedDtype {
                    dtype: def.dtype.clone(),
                })
            }
        };

        Ok(GeneratedGlobal { symbol, data })
    }

    fn handle_function(
        layouts: &StructLayouts,
        link_name: &str,
        body: &ir::FunctionBody,
        target: Target,
    ) -> Result<GeneratedFunction, Error> {
        let symbol = target.mangle_symbol(link_name);
        let mut frame = FrameLayout::from_blocks(&body.blocks, layouts)?;
        let mut insts = Self::handle_arguments(body)?;
        insts.extend(
            AsmFunctionGenerator::new(&symbol, &frame, layouts, target, body.next_vreg)
                .generate(&body.blocks)?,
        );

        let uses_fp = stream_uses_fp(&insts);
        let insts = RegisterAllocator::new(&insts, &mut frame).run()?;

        Ok(GeneratedFunction {
            symbol,
            frame_size: frame.frame_size(),
            insts,
            uses_fp,
        })
    }
}
