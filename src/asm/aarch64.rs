mod function_generator;
mod inst;
mod phi_lowering;
mod printer;
mod register_allocator;
mod types;

pub use inst::Inst;
pub use types::{Addr, BinOp, Cond, Operand, Register};

use crate::asm::common::{StackFrame, StructLayouts};
use crate::asm::error::Error;
use crate::common::{Generator, Target};
use crate::ir;
use function_generator::FunctionGenerator;
use printer::{AsmPrint, AsmPrinter};
use register_allocator::rewrite_insts;
use std::collections::HashMap;
use std::io::Write;
use types::dtype_to_regsize;

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
    insts: Vec<Inst>,
}

fn lower_link_symbol(ir_name: &str, target: Target) -> String {
    let base = if let Some(stripped) = ir_name.strip_prefix("std::") {
        stripped.to_string()
    } else {
        ir_name.replace("::", "__")
    };
    target.mangle_symbol(&base)
}

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
        for g in self.module.global_list.values() {
            self.globals
                .push(Self::handle_global(&layouts, g, self.target)?);
        }

        self.functions.clear();
        for func in self.module.function_list.values() {
            // Skip external declarations (blocks == None); they are provided by
            // the linked object file (e.g. std.o) and must not be emitted as
            // assembly symbols, otherwise the linker will report duplicate definitions.
            if func.blocks.is_none() {
                continue;
            }
            self.functions
                .push(Self::handle_function(&layouts, func, self.target)?);
        }

        Ok(())
    }

    fn output<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let mut printer = AsmPrinter::new(w, self.target);

        if !self.globals.is_empty() {
            printer.emit_section("data")?;
            for g in &self.globals {
                printer.emit_global(&g.symbol)?;
                printer.emit_align(2)?;
                printer.emit_label(&g.symbol)?;
                match &g.data {
                    GlobalData::Word { value } => printer.emit_word(*value)?,
                    GlobalData::Array { words, zero_bytes } => {
                        for v in words {
                            printer.emit_word(*v)?;
                        }
                        if *zero_bytes > 0 {
                            printer.emit_zero(*zero_bytes)?;
                        }
                    }
                }
            }
            printer.emit_newline()?;
        }

        printer.emit_section("text")?;
        for func in &self.functions {
            printer.emit_global(&func.symbol)?;
            printer.emit_align(2)?;
            printer.emit_label(&func.symbol)?;
            printer.emit_prologue(func.frame_size)?;
            printer.emit_insts(&func.insts)?;
            printer.emit_newline()?;
        }

        Ok(())
    }
}

impl<'a> AArch64AsmGenerator<'a> {
    fn handle_arguments(func: &ir::Function) -> Result<Vec<Inst>, Error> {
        let mut insts = Vec::new();

        for (i, arg) in func.arguments.iter().enumerate() {
            let v = arg.index;
            let size = dtype_to_regsize(&arg.dtype)?;

            if i < 8 {
                insts.push(Inst::Mov {
                    size,
                    dst: Register::Virtual(v),
                    src: Operand::Register(Register::Physical(i as u8)),
                });
            } else {
                // Stack arguments (9th onward) are above the saved fp/lr pair.
                // Stack layout after prologue:
                //   [fp+16]: arg 8, [fp+24]: arg 9, ...
                //   [fp+8]:  saved lr
                //   [fp]:    saved fp (frame pointer points here)
                let offset = 16 + ((i - 8) as i64) * 8;
                insts.push(Inst::Ldr {
                    size,
                    dst: Register::Virtual(v),
                    addr: Addr::BaseOff {
                        base: Register::Physical(29),
                        offset,
                    },
                });
            }
        }

        Ok(insts)
    }

    fn handle_global(
        layouts: &StructLayouts,
        g: &ir::GlobalVariable,
        target: Target,
    ) -> Result<GeneratedGlobal, Error> {
        let symbol = target.mangle_symbol(&g.identifier);

        let data = match &g.dtype {
            ir::Dtype::I32 => {
                let value = g
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

                if let Some(inits) = &g.initializers {
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
                    dtype: g.dtype.clone(),
                })
            }
        };

        Ok(GeneratedGlobal { symbol, data })
    }

    fn handle_function(
        layouts: &StructLayouts,
        func: &ir::Function,
        target: Target,
    ) -> Result<GeneratedFunction, Error> {
        let symbol = lower_link_symbol(&func.identifier, target);
        let Some(blocks) = func.blocks.as_ref() else {
            return Ok(GeneratedFunction {
                symbol,
                frame_size: 0,
                insts: Vec::new(),
            });
        };
        let mut frame = StackFrame::from_blocks(blocks, layouts)?;
        let mut next_vreg = func.next_vreg;
        let mut cond_map: HashMap<usize, Cond> = HashMap::new();
        let mut insts: Vec<Inst> = Vec::new();
        insts.extend(Self::handle_arguments(func)?);

        {
            let mut ctx = FunctionGenerator {
                func_id: &symbol,
                frame: &frame,
                layouts,
                target,
                insts: &mut insts,
                next_vreg: &mut next_vreg,
                cond_map: &mut cond_map,
            };
            phi_lowering::lower_function_blocks(&mut ctx, blocks)?;
        }

        let alloc = register_allocator::allocate(&insts);
        for v in alloc.spilled.iter().copied() {
            frame.alloc_spill(v, 8, 8);
        }
        let insts = rewrite_insts(&insts, &alloc, &frame)?;

        Ok(GeneratedFunction {
            symbol,
            frame_size: frame.frame_size_aligned(),
            insts,
        })
    }
}
