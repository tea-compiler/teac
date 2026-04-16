use super::inst::Inst;
use super::types::{dtype_to_regsize, Addr, BinOp, Cond, IndexOperand, Operand, RegSize, Register};
use crate::asm::common::{align_up, StackFrame, StackSlot, StructLayouts};
use crate::asm::error::Error;
use crate::common::Target;
use crate::ir;
use std::collections::HashMap;

fn mangle_bb(func: &str, bb: usize) -> String {
    format!(".L{func}_bb{bb}")
}

fn lower_link_symbol(ir_name: &str, target: Target) -> String {
    let base = if let Some(stripped) = ir_name.strip_prefix("std::") {
        stripped.to_string()
    } else {
        ir_name.replace("::", "__")
    };
    target.mangle_symbol(&base)
}

#[derive(Debug, Clone)]
pub enum PtrBase {
    Stack,
    Global(String),
    Register(usize),
}

pub struct FunctionGenerator<'a> {
    pub func_id: &'a str,
    pub frame: &'a StackFrame,
    pub layouts: &'a StructLayouts,
    pub target: Target,
    pub insts: &'a mut Vec<Inst>,
    pub next_vreg: &'a mut usize,
    pub cond_map: &'a mut HashMap<usize, Cond>,
}

impl<'a> FunctionGenerator<'a> {
    pub fn fresh_vreg(&mut self) -> usize {
        let v = *self.next_vreg;
        *self.next_vreg += 1;
        v
    }

    pub fn emit_label(&mut self, label: &ir::BlockLabel) {
        if let ir::BlockLabel::BasicBlock(n) = label {
            self.insts.push(Inst::Label(mangle_bb(self.func_id, *n)));
        }
    }

    pub fn emit_store(&mut self, s: &ir::stmt::StoreStmt) -> Result<(), Error> {
        let (src, size) = self.lower_value(&s.src)?;
        let addr = self.lower_ptr_as_addr(&s.ptr)?;

        match src {
            Operand::Register(r) => {
                self.insts.push(Inst::Str { size, src: r, addr });
            }
            Operand::Immediate(imm) => {
                let tmp = self.fresh_vreg();
                self.insts.push(Inst::Mov {
                    size,
                    dst: Register::Virtual(tmp),
                    src: Operand::Immediate(imm),
                });
                self.insts.push(Inst::Str {
                    size,
                    src: Register::Virtual(tmp),
                    addr,
                });
            }
        }
        Ok(())
    }

    pub fn emit_load(&mut self, s: &ir::stmt::LoadStmt) -> Result<(), Error> {
        let dst = Self::operand_vreg(&s.dst)?;
        let size = dtype_to_regsize(s.dst.dtype())?;

        let addr = self.lower_ptr_as_addr(&s.ptr)?;
        self.insts.push(Inst::Ldr {
            size,
            dst: Register::Virtual(dst),
            addr,
        });
        Ok(())
    }

    pub fn emit_biop(&mut self, s: &ir::stmt::BiOpStmt) -> Result<(), Error> {
        let dst = Self::operand_vreg(&s.dst)?;

        let lhs = self.lower_int_to_reg(&s.left)?;
        let rhs = self.lower_int(&s.right)?;
        let op = arith_op_to_binop(&s.kind);

        self.insts.push(Inst::BinOp {
            op,
            size: RegSize::W32,
            dst: Register::Virtual(dst),
            lhs,
            rhs,
        });
        Ok(())
    }

    pub fn emit_cmp(&mut self, s: &ir::stmt::CmpStmt) -> Result<(), Error> {
        let dst = Self::operand_vreg(&s.dst)?;
        let lhs = self.lower_int_to_reg(&s.left)?;
        let rhs = self.lower_int(&s.right)?;
        let cond = cmp_op_to_cond(&s.kind);

        self.cond_map.insert(dst, cond);
        self.insts.push(Inst::Cmp {
            size: RegSize::W32,
            lhs,
            rhs,
        });
        Ok(())
    }

    pub fn emit_cjump(&mut self, s: &ir::stmt::CJumpStmt) -> Result<(), Error> {
        let cond_v = Self::operand_vreg(&s.cond)?;
        let cond = *self
            .cond_map
            .get(&cond_v)
            .ok_or(Error::MissingCond { vreg: cond_v })?;

        let true_label = self.mangle_block_label(&s.true_label);
        let false_label = self.mangle_block_label(&s.false_label);

        self.insts.push(Inst::BCond {
            cond,
            label: true_label,
        });
        self.insts.push(Inst::B { label: false_label });
        Ok(())
    }

    pub fn emit_jump(&mut self, s: &ir::stmt::JumpStmt) {
        let target = self.mangle_block_label(&s.target);
        self.insts.push(Inst::B { label: target });
    }

    pub fn emit_gep(&mut self, s: &ir::stmt::GepStmt) -> Result<(), Error> {
        let new_ptr = Self::operand_vreg(&s.new_ptr)?;

        let (base_kind, base_slot) = self.lower_ptr(&s.base_ptr)?;

        match s.base_ptr.dtype() {
            ir::Dtype::Pointer { pointee } => {
                let is_struct_field_access = matches!(pointee.as_ref(), ir::Dtype::Struct { .. })
                    && s.new_ptr.dtype() != s.base_ptr.dtype();

                if is_struct_field_access {
                    if let ir::Dtype::Struct { type_name } = pointee.as_ref() {
                        return self
                            .emit_gep_struct(new_ptr, &s.index, type_name, base_kind, base_slot);
                    }
                }
                let elem = match pointee.as_ref() {
                    ir::Dtype::Array { element, .. } => element.as_ref(),
                    other => other,
                };
                self.emit_gep_array(new_ptr, &s.index, elem, base_kind, base_slot)
            }
            ir::Dtype::Array { element, .. } => {
                self.emit_gep_array(new_ptr, &s.index, element.as_ref(), base_kind, base_slot)
            }
            other => Err(Error::UnsupportedDtype {
                dtype: other.clone(),
            }),
        }
    }

    pub fn emit_call(&mut self, s: &ir::stmt::CallStmt) -> Result<(), Error> {
        self.insts.push(Inst::SaveCallerRegs);

        let nargs = s.args.len();
        if nargs > 8 {
            let stack_bytes = align_up(((nargs - 8) as i64) * 8, 16);
            self.insts.push(Inst::SubSp { imm: stack_bytes });

            for (i, arg) in s.args.iter().enumerate().skip(8) {
                self.emit_call_stack_arg(arg, ((i - 8) as i64) * 8)?;
            }
        }

        for (i, arg) in s.args.iter().enumerate().take(8) {
            self.emit_call_reg_arg(arg, i as u8)?;
        }

        let func_name = lower_link_symbol(&s.func_name, self.target);
        self.insts.push(Inst::Bl { func: func_name });

        if nargs > 8 {
            let stack_bytes = align_up(((nargs - 8) as i64) * 8, 16);
            self.insts.push(Inst::AddSp { imm: stack_bytes });
        }

        self.insts.push(Inst::RestoreCallerRegs);

        if let Some(res) = &s.res {
            match res {
                ir::Operand::Local(local) => self.emit_call_result(local)?,
                _ => {
                    return Err(Error::Internal(
                        "call result must be a local operand".into(),
                    ))
                }
            }
        }
        Ok(())
    }

    pub fn emit_return(&mut self, s: &ir::stmt::ReturnStmt) -> Result<(), Error> {
        if let Some(v) = &s.val {
            let (op, size) = self.lower_value(v)?;
            self.insts.push(Inst::Mov {
                size,
                dst: Register::Physical(0),
                src: op,
            });
        }
        self.insts.push(Inst::Ret);
        Ok(())
    }

    fn emit_gep_struct(
        &mut self,
        new_ptr: usize,
        idx: &ir::Operand,
        type_name: &str,
        base_kind: PtrBase,
        base_slot: Option<StackSlot>,
    ) -> Result<(), Error> {
        let field_index = self.lower_index_imm(idx)?;
        let layout = self
            .layouts
            .get(type_name)
            .ok_or_else(|| Error::MissingStructLayout {
                name: type_name.to_string(),
            })?;

        let fi = field_index as usize;
        if fi >= layout.field_offsets.len() {
            return Err(Error::InvalidStructFieldIndex {
                name: type_name.to_string(),
                index: field_index,
            });
        }
        let offset = layout.field_offsets[fi];

        self.emit_ptr_offset(new_ptr, base_kind, base_slot, offset)
    }

    fn emit_gep_array(
        &mut self,
        new_ptr: usize,
        idx: &ir::Operand,
        inner: &ir::Dtype,
        base_kind: PtrBase,
        base_slot: Option<StackSlot>,
    ) -> Result<(), Error> {
        let (elem_size, _) = self.layouts.size_align_of(inner)?;
        let index = self.lower_index(idx)?;

        match (base_kind, base_slot) {
            (PtrBase::Stack, Some(slot)) => {
                self.insts.push(Inst::Lea {
                    dst: Register::Virtual(new_ptr),
                    addr: Addr::BaseOff {
                        base: Register::Physical(29),
                        offset: slot.offset_from_fp,
                    },
                });
                self.insts.push(Inst::Gep {
                    dst: Register::Virtual(new_ptr),
                    base: Register::Virtual(new_ptr),
                    index,
                    scale: elem_size,
                });
            }
            (PtrBase::Stack, None) => {
                return Err(Error::Internal(
                    "missing stack slot for stack pointer".into(),
                ));
            }
            (PtrBase::Global(sym), _) => {
                self.insts.push(Inst::Lea {
                    dst: Register::Virtual(new_ptr),
                    addr: Addr::Global(sym),
                });
                self.insts.push(Inst::Gep {
                    dst: Register::Virtual(new_ptr),
                    base: Register::Virtual(new_ptr),
                    index,
                    scale: elem_size,
                });
            }
            (PtrBase::Register(base_v), _) => {
                self.insts.push(Inst::Gep {
                    dst: Register::Virtual(new_ptr),
                    base: Register::Virtual(base_v),
                    index,
                    scale: elem_size,
                });
            }
        }
        Ok(())
    }

    fn emit_ptr_offset(
        &mut self,
        dst: usize,
        base_kind: PtrBase,
        base_slot: Option<StackSlot>,
        offset: i64,
    ) -> Result<(), Error> {
        match (base_kind, base_slot) {
            (PtrBase::Stack, Some(slot)) => {
                self.insts.push(Inst::Lea {
                    dst: Register::Virtual(dst),
                    addr: Addr::BaseOff {
                        base: Register::Physical(29),
                        offset: slot.offset_from_fp + offset,
                    },
                });
            }
            (PtrBase::Stack, None) => {
                return Err(Error::Internal(
                    "missing stack slot for stack pointer".into(),
                ));
            }
            (PtrBase::Global(sym), _) => {
                self.insts.push(Inst::Lea {
                    dst: Register::Virtual(dst),
                    addr: Addr::Global(sym),
                });
                if offset != 0 {
                    self.insts.push(Inst::BinOp {
                        op: BinOp::Add,
                        size: RegSize::X64,
                        dst: Register::Virtual(dst),
                        lhs: Register::Virtual(dst),
                        rhs: Operand::Immediate(offset),
                    });
                }
            }
            (PtrBase::Register(base_v), _) => {
                self.insts.push(Inst::Lea {
                    dst: Register::Virtual(dst),
                    addr: Addr::BaseOff {
                        base: Register::Virtual(base_v),
                        offset,
                    },
                });
            }
        }
        Ok(())
    }

    fn emit_call_stack_arg(&mut self, arg: &ir::Operand, stack_offset: i64) -> Result<(), Error> {
        if matches!(
            arg.dtype(),
            ir::Dtype::Pointer { .. } | ir::Dtype::Array { .. }
        ) {
            self.emit_ptr_to_reg(arg, Register::Physical(16))?;
            self.insts.push(Inst::Str {
                size: RegSize::X64,
                src: Register::Physical(16),
                addr: Addr::BaseOff {
                    base: Register::StackPointer,
                    offset: stack_offset,
                },
            });
        } else {
            let (op, _size) = self.lower_value(arg)?;
            match op {
                Operand::Immediate(imm) => {
                    self.insts.push(Inst::Mov {
                        size: RegSize::W32,
                        dst: Register::Physical(16),
                        src: Operand::Immediate(imm),
                    });
                    self.insts.push(Inst::Str {
                        size: RegSize::W32,
                        src: Register::Physical(16),
                        addr: Addr::BaseOff {
                            base: Register::StackPointer,
                            offset: stack_offset,
                        },
                    });
                }
                Operand::Register(r) => {
                    self.insts.push(Inst::Str {
                        size: RegSize::W32,
                        src: r,
                        addr: Addr::BaseOff {
                            base: Register::StackPointer,
                            offset: stack_offset,
                        },
                    });
                }
            }
        }
        Ok(())
    }

    fn emit_call_reg_arg(&mut self, arg: &ir::Operand, reg_idx: u8) -> Result<(), Error> {
        if matches!(
            arg.dtype(),
            ir::Dtype::Pointer { .. } | ir::Dtype::Array { .. }
        ) {
            self.emit_ptr_to_reg(arg, Register::Physical(reg_idx))?;
        } else {
            let (op, _size) = self.lower_value(arg)?;
            self.insts.push(Inst::Mov {
                size: RegSize::W32,
                dst: Register::Physical(reg_idx),
                src: op,
            });
        }
        Ok(())
    }

    fn emit_call_result(&mut self, res: &ir::Local) -> Result<(), Error> {
        let dst = res.id.0;
        match &res.dtype {
            ir::Dtype::I32 => {
                self.insts.push(Inst::Mov {
                    size: RegSize::W32,
                    dst: Register::Virtual(dst),
                    src: Operand::Register(Register::Physical(0)),
                });
                Ok(())
            }
            other => Err(Error::UnsupportedDtype {
                dtype: other.clone(),
            }),
        }
    }

    fn emit_ptr_to_reg(&mut self, arg: &ir::Operand, dst: Register) -> Result<(), Error> {
        let (base_kind, slot) = self.lower_ptr(arg)?;
        match base_kind {
            PtrBase::Register(v) => {
                self.insts.push(Inst::Mov {
                    size: RegSize::X64,
                    dst,
                    src: Operand::Register(Register::Virtual(v)),
                });
            }
            PtrBase::Stack => {
                let slot = slot.ok_or_else(|| Error::Internal("missing stack slot".into()))?;
                self.insts.push(Inst::Lea {
                    dst,
                    addr: Addr::BaseOff {
                        base: Register::Physical(29),
                        offset: slot.offset_from_fp,
                    },
                });
            }
            PtrBase::Global(sym) => {
                self.insts.push(Inst::Lea {
                    dst,
                    addr: Addr::Global(sym),
                });
            }
        }
        Ok(())
    }

    fn lower_int(&self, val: &ir::Operand) -> Result<Operand, Error> {
        match val {
            ir::Operand::Const(c) => Ok(Operand::Immediate(c.val)),
            ir::Operand::Local(l) => {
                if !matches!(l.dtype, ir::Dtype::I1 | ir::Dtype::I32) {
                    return Err(Error::UnsupportedDtype {
                        dtype: l.dtype.clone(),
                    });
                }
                if self.frame.has_alloca(l.id.0) {
                    return Err(Error::UnsupportedOperand {
                        what: format!("int operand references alloca pointer %r{}", l.id.0),
                    });
                }
                Ok(Operand::Register(Register::Virtual(l.id.0)))
            }
            ir::Operand::Global(_) => Err(Error::UnsupportedOperand {
                what: format!("unsupported int operand: {}", val),
            }),
        }
    }

    fn lower_int_to_reg(&mut self, val: &ir::Operand) -> Result<Register, Error> {
        match self.lower_int(val)? {
            Operand::Register(r) => Ok(r),
            Operand::Immediate(imm) => {
                let tmp = self.fresh_vreg();
                self.insts.push(Inst::Mov {
                    size: RegSize::W32,
                    dst: Register::Virtual(tmp),
                    src: Operand::Immediate(imm),
                });
                Ok(Register::Virtual(tmp))
            }
        }
    }

    fn lower_value(&self, val: &ir::Operand) -> Result<(Operand, RegSize), Error> {
        match val {
            ir::Operand::Const(c) => Ok((Operand::Immediate(c.val), RegSize::W32)),
            ir::Operand::Local(l) => {
                let size = match &l.dtype {
                    ir::Dtype::I1 | ir::Dtype::I32 => RegSize::W32,
                    ir::Dtype::Pointer { .. } => {
                        if self.frame.has_alloca(l.id.0) {
                            return Err(Error::UnsupportedOperand {
                                what: format!(
                                    "value operand uses alloca ptr %r{} directly (need address-of)",
                                    l.id.0
                                ),
                            });
                        }
                        RegSize::X64
                    }
                    other => {
                        return Err(Error::UnsupportedDtype {
                            dtype: other.clone(),
                        })
                    }
                };
                Ok((Operand::Register(Register::Virtual(l.id.0)), size))
            }
            ir::Operand::Global(_) => Err(Error::UnsupportedOperand {
                what: "unexpected global variable in value position".into(),
            }),
        }
    }

    fn lower_ptr_as_addr(&self, val: &ir::Operand) -> Result<Addr, Error> {
        let (base_kind, slot) = self.lower_ptr(val)?;
        match base_kind {
            PtrBase::Stack => {
                let slot = slot.ok_or_else(|| Error::Internal("missing stack slot".into()))?;
                Ok(Addr::BaseOff {
                    base: Register::Physical(29),
                    offset: slot.offset_from_fp,
                })
            }
            PtrBase::Global(sym) => Ok(Addr::Global(sym)),
            PtrBase::Register(v) => Ok(Addr::BaseOff {
                base: Register::Virtual(v),
                offset: 0,
            }),
        }
    }

    fn lower_ptr(&self, val: &ir::Operand) -> Result<(PtrBase, Option<StackSlot>), Error> {
        match val {
            ir::Operand::Local(l) => {
                let vreg_index = l.id.0;
                // Check if this local is a stack allocation (alloca).
                // Allocas have their address implicitly defined by their stack slot,
                // rather than being stored in a register.
                if let Some(slot) = self.frame.alloca_slot(vreg_index) {
                    return Ok((PtrBase::Stack, Some(slot)));
                }
                // Otherwise, if it's a pointer type, the pointer value itself
                // lives in a virtual register (e.g., result of a GEP or load).
                if matches!(l.dtype, ir::Dtype::Pointer { .. }) {
                    return Ok((PtrBase::Register(vreg_index), None));
                }
                // Non-pointer locals cannot be used as pointer operands.
                Err(Error::UnsupportedDtype {
                    dtype: l.dtype.clone(),
                })
            }
            ir::Operand::Global(g) => Ok((
                PtrBase::Global(self.target.mangle_symbol(&g.name)),
                None,
            )),
            ir::Operand::Const(_) => Err(Error::UnsupportedOperand {
                what: format!("unsupported pointer operand: {}", val),
            }),
        }
    }

    fn lower_index(&self, val: &ir::Operand) -> Result<IndexOperand, Error> {
        match val {
            ir::Operand::Const(c) => Ok(IndexOperand::Imm(c.val)),
            ir::Operand::Local(l) => {
                if !matches!(l.dtype, ir::Dtype::I1 | ir::Dtype::I32) {
                    return Err(Error::UnsupportedDtype {
                        dtype: l.dtype.clone(),
                    });
                }
                if self.frame.has_alloca(l.id.0) {
                    return Err(Error::UnsupportedOperand {
                        what: format!("index operand references alloca pointer %r{}", l.id.0),
                    });
                }
                Ok(IndexOperand::Reg(Register::Virtual(l.id.0)))
            }
            ir::Operand::Global(_) => Err(Error::UnsupportedOperand {
                what: format!("unsupported index operand: {}", val),
            }),
        }
    }

    fn lower_index_imm(&self, val: &ir::Operand) -> Result<i64, Error> {
        match val {
            ir::Operand::Const(c) => Ok(c.val),
            _ => Err(Error::UnsupportedOperand {
                what: format!("expected immediate struct field index, got: {}", val),
            }),
        }
    }

    pub fn emit_stmt(&mut self, stmt: &ir::stmt::Stmt) -> Result<(), Error> {
        use ir::stmt::StmtInner::*;
        match &stmt.inner {
            Label(l) => {
                self.emit_label(&l.label);
                Ok(())
            }
            Alloca(_) => Ok(()),
            Store(s) => self.emit_store(s),
            Load(s) => self.emit_load(s),
            BiOp(s) => self.emit_biop(s),
            Cmp(s) => self.emit_cmp(s),
            CJump(s) => self.emit_cjump(s),
            Jump(s) => {
                self.emit_jump(s);
                Ok(())
            }
            Gep(s) => self.emit_gep(s),
            Call(s) => self.emit_call(s),
            Return(s) => self.emit_return(s),
            Phi(_) => Err(Error::Internal(
                "phi nodes should be lowered before assembly emission".into(),
            )),
        }
    }

    pub fn emit_copy(&mut self, dst: &ir::Operand, src: &ir::Operand) -> Result<(), Error> {
        let dst_vreg = Self::operand_vreg(dst)?;
        let size = dtype_to_regsize(dst.dtype())?;

        let src_op = match src {
            ir::Operand::Const(c) => Operand::Immediate(c.val),
            ir::Operand::Local(l) => Operand::Register(Register::Virtual(l.id.0)),
            ir::Operand::Global(_) => {
                return Err(Error::UnsupportedOperand {
                    what: "global variable in phi copy".into(),
                });
            }
        };

        self.insts.push(Inst::Mov {
            size,
            dst: Register::Virtual(dst_vreg),
            src: src_op,
        });
        Ok(())
    }

    fn mangle_block_label(&self, label: &ir::BlockLabel) -> String {
        match label {
            ir::BlockLabel::BasicBlock(n) => mangle_bb(self.func_id, *n),
            ir::BlockLabel::Function(name) => name.clone(),
        }
    }

    fn operand_vreg(op: &ir::Operand) -> Result<usize, Error> {
        op.local_id()
            .map(|id| id.0)
            .ok_or_else(|| Error::UnsupportedOperand {
                what: format!("expected local variable, got: {}", op),
            })
    }
}

fn arith_op_to_binop(op: &ir::stmt::ArithBinOp) -> BinOp {
    match op {
        ir::stmt::ArithBinOp::Add => BinOp::Add,
        ir::stmt::ArithBinOp::Sub => BinOp::Sub,
        ir::stmt::ArithBinOp::Mul => BinOp::Mul,
        ir::stmt::ArithBinOp::SDiv => BinOp::SDiv,
    }
}

fn cmp_op_to_cond(op: &ir::stmt::CmpPredicate) -> Cond {
    match op {
        ir::stmt::CmpPredicate::Eq => Cond::Eq,
        ir::stmt::CmpPredicate::Ne => Cond::Ne,
        ir::stmt::CmpPredicate::Slt => Cond::Lt,
        ir::stmt::CmpPredicate::Sle => Cond::Le,
        ir::stmt::CmpPredicate::Sgt => Cond::Gt,
        ir::stmt::CmpPredicate::Sge => Cond::Ge,
    }
}
