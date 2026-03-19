use crate::ast;
use crate::ir::function::{BasicBlock, BlockLabel, Function, FunctionGenerator};
use crate::ir::module::IrGenerator;
use crate::ir::printer::IrPrinter;
use crate::ir::stmt::{Stmt, StmtInner};
use crate::ir::types::{Dtype, FunctionType, StructMember, StructType};
use crate::ir::value::GlobalVariable;
use crate::ir::Error;

use crate::common::Generator;
use crate::ir::value::Named;
use std::io::Write;

impl<'a> Generator for IrGenerator<'a> {
    type Error = Error;

    fn generate(&mut self) -> Result<(), Error> {
        let input = self.input;

        for use_stmt in input.use_stmts.iter() {
            self.handle_use_stmt(use_stmt)?;
        }

        for elem in input.elements.iter() {
            use ast::ProgramElementInner::*;
            match &elem.inner {
                VarDeclStmt(stmt) => self.handle_global_var_decl(stmt)?,
                FnDeclStmt(fn_decl) => self.handle_fn_decl(fn_decl)?,
                FnDef(fn_def) => self.handle_fn_def(fn_def)?,
                StructDef(struct_def) => self.handle_struct_def(struct_def)?,
            }
        }

        for elem in input.elements.iter() {
            use ast::ProgramElementInner::*;
            if let FnDef(fn_def) = &elem.inner {
                let (next_vreg, blocks, local_variables, arguments) = {
                    let mut function_generator =
                        FunctionGenerator::new(&self.registry, &self.module.global_list);
                    function_generator.generate(fn_def)?;

                    let next_vreg = function_generator.next_vreg;
                    let blocks = Self::harvest_function_irs(function_generator.irs);
                    let local_variables = function_generator.local_variables;
                    let arguments = function_generator.arguments;
                    (next_vreg, blocks, local_variables, arguments)
                };

                let func = self
                    .module
                    .function_list
                    .get_mut(&fn_def.fn_decl.identifier);

                if let Some(f) = func {
                    f.blocks = Some(blocks);
                    f.local_variables = Some(local_variables);
                    f.arguments = arguments;
                    f.next_vreg = next_vreg;
                } else {
                    return Err(Error::FunctionNotDefined {
                        symbol: fn_def.fn_decl.identifier.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    fn output<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let mut printer = IrPrinter::new(w);

        printer.emit_header(Self::TARGET_TRIPLE, Self::TARGET_DATALAYOUT)?;

        for (name, st) in self.registry.struct_types.iter() {
            printer.emit_struct_type(name, st)?;
        }
        printer.emit_newline()?;

        for global in self.module.global_list.values() {
            printer.emit_global(global)?;
        }
        printer.emit_newline()?;

        for func in self.module.function_list.values() {
            let func_type = self
                .registry
                .function_types
                .get(&func.identifier)
                .ok_or_else(|| Error::FunctionNotDefined {
                    symbol: func.identifier.clone(),
                })?;
            if let Some(blocks) = &func.blocks {
                printer.emit_function_def(func, &func_type.return_dtype, blocks)?;
            } else {
                printer.emit_function_decl(&func.identifier, func_type)?;
            }
        }

        Ok(())
    }
}

impl<'a> IrGenerator<'a> {
    fn handle_use_stmt(&mut self, use_stmt: &ast::UseStmt) -> Result<(), Error> {
        if use_stmt.module_name == "std" {
            self.register_std_functions()?;
        }
        Ok(())
    }

    fn register_std_functions(&mut self) -> Result<(), Error> {
        let std_functions = vec![
            ("std::getint", vec![], Dtype::I32),
            ("std::getch", vec![], Dtype::I32),
            (
                "std::putint",
                vec![("a".to_string(), Dtype::I32)],
                Dtype::Void,
            ),
            (
                "std::putch",
                vec![("a".to_string(), Dtype::I32)],
                Dtype::Void,
            ),
            (
                "std::timer_start",
                vec![("lineno".to_string(), Dtype::I32)],
                Dtype::Void,
            ),
            (
                "std::timer_stop",
                vec![("lineno".to_string(), Dtype::I32)],
                Dtype::Void,
            ),
            (
                "std::putarray",
                vec![
                    ("n".to_string(), Dtype::I32),
                    (
                        "a".to_string(),
                        Dtype::ptr_to(Dtype::Array {
                            element: Box::new(Dtype::I32),
                            length: None,
                        }),
                    ),
                ],
                Dtype::Void,
            ),
        ];

        for (name, arguments, return_dtype) in std_functions {
            self.registry.function_types.insert(
                name.to_string(),
                FunctionType {
                    return_dtype,
                    arguments,
                },
            );
        }

        Ok(())
    }

    fn harvest_function_irs(irs: Vec<Stmt>) -> Vec<BasicBlock> {
        let mut blocks = Vec::new();
        let mut label: Option<BlockLabel> = None;
        let mut stmts = Vec::new();
        let mut terminated = false;

        for stmt in irs {
            match &stmt.inner {
                StmtInner::Label(l) => {
                    if let Some(prev_label) = label.take() {
                        blocks.push(BasicBlock {
                            label: prev_label,
                            stmts: std::mem::take(&mut stmts),
                        });
                    }
                    label = Some(l.label.clone());
                    terminated = false;
                }
                _ => {
                    if label.is_none() || terminated {
                        continue;
                    }
                    terminated = matches!(
                        &stmt.inner,
                        StmtInner::Return(_) | StmtInner::CJump(_) | StmtInner::Jump(_)
                    );
                    stmts.push(stmt);
                }
            }
        }
        if let Some(last_label) = label {
            blocks.push(BasicBlock {
                label: last_label,
                stmts,
            });
        }

        if blocks.is_empty() {
            return blocks;
        }

        // Hoist all allocas from non-entry blocks to the entry block, right
        // after the entry label.  This ensures all stack allocations happen in
        // the entry block (LLVM convention).
        let mut hoisted_allocas: Vec<Stmt> = Vec::new();
        for block in blocks.iter_mut().skip(1) {
            let (allocas, remaining): (Vec<Stmt>, Vec<Stmt>) = block
                .stmts
                .drain(..)
                .partition(|x| matches!(&x.inner, StmtInner::Alloca(_)));
            hoisted_allocas.extend(allocas);
            block.stmts = remaining;
        }
        // Insert hoisted allocas at the beginning of the entry block.
        blocks[0].stmts.splice(0..0, hoisted_allocas);

        // Remove blocks that became empty (only a label, no terminator) after
        // alloca hoisting, as they violate the basic block invariant.
        blocks.retain(|block| !block.stmts.is_empty());

        blocks
    }

    fn handle_global_var_decl(&mut self, stmt: &ast::VarDeclStmt) -> Result<(), Error> {
        let identifier = match stmt.identifier() {
            Some(id) => id,
            None => return Err(Error::SymbolMissing),
        };

        let dtype = Dtype::try_from(stmt)?;
        let initializers = if let ast::VarDeclStmtInner::Def(d) = &stmt.inner {
            Some(match &d.inner {
                ast::VarDefInner::Array(def) => match &def.initializer {
                    ast::ArrayInitializer::ExplicitList(vals) => vals
                        .iter()
                        .map(Self::handle_right_val_static)
                        .collect::<Result<Vec<_>, _>>()?,
                    ast::ArrayInitializer::Fill { val, count } => {
                        let v = Self::handle_right_val_static(val)?;
                        vec![v; *count]
                    }
                },
                ast::VarDefInner::Scalar(scalar) => {
                    let value = Self::handle_right_val_static(&scalar.val)?;
                    vec![value]
                }
            })
        } else {
            None
        };

        self.module
            .global_list
            .insert(
                identifier.clone(),
                GlobalVariable {
                    dtype,
                    identifier,
                    initializers,
                },
            )
            .map_or(Ok(()), |v| {
                Err(Error::VariableRedefinition {
                    symbol: v.identifier,
                })
            })
    }

    fn handle_fn_decl(&mut self, decl: &ast::FnDecl) -> Result<(), Error> {
        let identifier = decl.identifier.clone();

        let mut arguments = Vec::new();
        if let Some(params) = &decl.param_decl {
            for decl in params.decls.iter() {
                let id = decl.identifier().ok_or(Error::SymbolMissing)?;
                let dtype = Dtype::try_from(decl)?;
                if matches!(&dtype, Dtype::Array { .. }) {
                    return Err(Error::ArrayParameterNotAllowed { symbol: id });
                }
                arguments.push((id, dtype));
            }
        }

        let function_type = FunctionType {
            return_dtype: match decl.return_dtype.as_ref() {
                Some(type_specifier) => type_specifier.into(),
                None => Dtype::Void,
            },
            arguments,
        };

        if let Some(ftype) = self
            .registry
            .function_types
            .insert(identifier.clone(), function_type.clone())
        {
            if ftype != function_type {
                return Err(Error::ConflictedFunction { symbol: identifier });
            }
        }

        self.module.function_list.insert(
            identifier.clone(),
            Function {
                arguments: Vec::new(),
                local_variables: None,
                identifier: identifier.clone(),
                blocks: None,
                next_vreg: 0,
            },
        );

        Ok(())
    }

    fn handle_fn_def(&mut self, stmt: &ast::FnDef) -> Result<(), Error> {
        let identifier = stmt.fn_decl.identifier.clone();

        match self.registry.function_types.get(&identifier) {
            None => self.handle_fn_decl(&stmt.fn_decl)?,
            Some(ty) => {
                if ty != stmt.fn_decl.as_ref() {
                    return Err(Error::DeclDefMismatch {
                        symbol: identifier.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    fn handle_struct_def(&mut self, struct_def: &ast::StructDef) -> Result<(), Error> {
        let identifier = struct_def.identifier.clone();
        let mut elements = Vec::new();

        for (offset, decl) in struct_def.decls.iter().enumerate() {
            let base_dtype = match decl.type_specifier.as_ref() {
                Some(type_specifier) => type_specifier.into(),
                None => Dtype::Void,
            };

            if let Dtype::Struct { type_name } = &base_dtype {
                if !self.registry.struct_types.contains_key(type_name) {
                    return Err(Error::UndefinedStructMemberType {
                        struct_name: identifier.clone(),
                        member_type: type_name.clone(),
                    });
                }
            }

            elements.push((
                decl.identifier.clone(),
                StructMember {
                    offset: offset as i32,
                    dtype: match &decl.inner {
                        ast::VarDeclInner::Scalar => base_dtype,
                        ast::VarDeclInner::Array(array) => Dtype::array_of(base_dtype, array.len),
                    },
                },
            ));
        }

        self.registry
            .struct_types
            .insert(identifier, StructType { elements });

        Ok(())
    }
}
