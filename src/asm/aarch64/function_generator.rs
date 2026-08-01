//! Instruction selection: lowers IR statements to AArch64 instructions
//! over virtual registers, including ABI handling for arguments, calls,
//! and returns.

use super::aapcs::{classify_args, ArgumentLocation};
use super::frame::{outgoing_arg_addr, outgoing_stack_bytes, FrameLayout};
use super::inst::Instruction;
use super::phi_lowering::{self, ParallelCopy, SplitEdge};
use super::types::{
    Addr, Cond, InstBinOp, InstOperand, Register, RegisterSize, REG_IP0, REG_X0,
};
use crate::asm::common::{StackSlot, StructLayouts};
use crate::asm::error::Error;
use crate::common::Target;
use crate::ir;
use std::collections::HashMap;

fn mangle_bb(func: &str, bb: usize) -> String {
    format!(".L{func}_bb{bb}")
}

fn is_terminator(stmt: &ir::stmt::Stmt) -> bool {
    matches!(
        stmt.inner,
        ir::stmt::StmtInner::Jump(_) | ir::stmt::StmtInner::CJump(_) | ir::stmt::StmtInner::Return(_)
    )
}

#[derive(Debug, Clone)]
pub enum PtrBase {
    Stack,
    Global(String),
    Register(usize),
}

pub struct AsmFunctionGenerator<'a> {
    func_id: &'a str,
    frame: &'a FrameLayout,
    layouts: &'a StructLayouts,
    target: Target,
    insts: Vec<Instruction>,
    next_vreg: usize,
    cond_map: HashMap<usize, Cond>,
}

impl<'a> AsmFunctionGenerator<'a> {
    /// Creates a generator for one function body.  `next_vreg` seeds the
    /// virtual-register counter from the IR body's high-water mark so the
    /// copies introduced during phi lowering get fresh ids.
    pub fn new(
        func_id: &'a str,
        frame: &'a FrameLayout,
        layouts: &'a StructLayouts,
        target: Target,
        next_vreg: usize,
    ) -> Self {
        Self {
            func_id,
            frame,
            layouts,
            target,
            insts: Vec::new(),
            next_vreg,
            cond_map: HashMap::new(),
        }
    }

    /// Lowers `blocks` to the function's flat instruction stream: each
    /// block is emitted in order with its phi-derived edge copies woven
    /// in before the terminator, then the synthesised split-edge blocks
    /// are appended.  The phi placement and edge splitting are computed
    /// by [`phi_lowering::plan`]; this method owns the walk and emission.
    /// Consumes the generator and returns the produced instructions.
    pub fn generate(mut self, blocks: &[ir::function::BasicBlock]) -> Result<Vec<Instruction>, Error> {
        let plan = phi_lowering::plan(blocks);

        for (idx, block) in plan.blocks.iter().enumerate() {
            self.emit_label(&block.label);
            self.emit_block_body(&block.body, plan.pending_inserts.get(&idx).map(Vec::as_slice))?;
        }
        self.emit_split_edges(&plan.splits)?;

        Ok(self.insts)
    }

    /// Emits a block body, injecting the edge copies immediately before
    /// the block's terminator (or at the end when the block has none).
    fn emit_block_body(
        &mut self,
        body: &[ir::stmt::Stmt],
        edge_copies: Option<&[ParallelCopy]>,
    ) -> Result<(), Error> {
        match body.iter().rposition(is_terminator) {
            Some(pos) => {
                for stmt in &body[..pos] {
                    self.emit_stmt(stmt)?;
                }
                self.emit_parallel_copies(edge_copies)?;
                for stmt in &body[pos..] {
                    self.emit_stmt(stmt)?;
                }
            }
            None => {
                for stmt in body {
                    self.emit_stmt(stmt)?;
                }
                self.emit_parallel_copies(edge_copies)?;
            }
        }
        Ok(())
    }

    /// Emits the synthesised split-edge blocks: each carries its phi
    /// copies followed by an unconditional jump to the original successor.
    fn emit_split_edges(&mut self, splits: &[SplitEdge]) -> Result<(), Error> {
        for split in splits {
            self.emit_label(&split.label);
            self.emit_parallel_copies(Some(&split.copies))?;
            self.emit_stmt(&ir::stmt::Stmt::as_jump(split.succ_label.clone()))?;
        }
        Ok(())
    }

    /// Sequences a parallel-copy bundle into individual moves: copies
    /// whose destination feeds no other pending copy go first; a residual
    /// cycle is broken by routing one destination through a fresh temp.
    fn emit_parallel_copies(&mut self, copies: Option<&[ParallelCopy]>) -> Result<(), Error> {
        let Some(copies) = copies else {
            return Ok(());
        };

        let mut pending = copies.to_vec();
        while !pending.is_empty() {
            if let Some(idx) = phi_lowering::find_ready_copy(&pending) {
                let copy = pending.remove(idx);
                self.emit_copy(&copy.dst, &copy.src)?;
                continue;
            }

            let cycle_dst = pending[0].dst.clone();
            let temp = ir::Operand::from(ir::Local::new(
                cycle_dst.dtype().clone(),
                ir::LocalId(self.fresh_vreg()),
            ));
            self.emit_copy(&temp, &cycle_dst)?;

            for copy in &mut pending {
                if phi_lowering::same_operand(&copy.src, &cycle_dst) {
                    copy.src = temp.clone();
                }
            }
        }

        Ok(())
    }

    pub fn fresh_vreg(&mut self) -> usize {
        let v = self.next_vreg;
        self.next_vreg += 1;
        v
    }

    pub fn emit_label(&mut self, label: &ir::BlockLabel) {
        if let ir::BlockLabel::BasicBlock(n) = label {
            self.insts.push(Instruction::Label(mangle_bb(self.func_id, *n)));
        }
    }

    pub fn emit_store(&mut self, s: &ir::stmt::StoreStmt) -> Result<(), Error> {
        let (src, size) = self.lower_value(&s.src)?;
        let addr = self.lower_ptr_as_addr(&s.ptr)?;

        match src {
            InstOperand::Register(r) => {
                self.insts.push(Instruction::Str { size, src: r, addr });
            }
            InstOperand::Immediate(imm) => {
                let tmp = self.fresh_vreg();
                self.insts.push(Instruction::Mov {
                    size,
                    dst: Register::Virtual(tmp),
                    src: InstOperand::Immediate(imm),
                });
                self.insts.push(Instruction::Str {
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
        let size = RegisterSize::try_from(s.dst.dtype())?;

        let addr = self.lower_ptr_as_addr(&s.ptr)?;
        self.insts.push(Instruction::Ldr {
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

        self.insts.push(Instruction::BinOp {
            op,
            size: RegisterSize::W32,
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
        self.insts.push(Instruction::Cmp {
            size: RegisterSize::W32,
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

        self.insts.push(Instruction::BCond {
            cond,
            label: true_label,
        });
        self.insts.push(Instruction::B { label: false_label });
        Ok(())
    }

    pub fn emit_jump(&mut self, s: &ir::stmt::JumpStmt) {
        let target = self.mangle_block_label(&s.target);
        self.insts.push(Instruction::B { label: target });
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
        let locs = classify_args(s.args.iter().map(ir::Operand::dtype))?;
        let stack_bytes = outgoing_stack_bytes(&locs);

        self.insts.push(Instruction::SaveCallerRegs);

        if stack_bytes > 0 {
            self.insts.push(Instruction::SubSp { imm: stack_bytes });
        }

        // Stack arguments are settled before register arguments: the
        // stack-store path borrows `x16` as scratch, which would
        // clobber any value already settled into `x0..x7`.
        for (arg, loc) in s.args.iter().zip(&locs) {
            if let ArgumentLocation::Stack { offset } = *loc {
                self.emit_stack_arg(arg, offset)?;
            }
        }

        for (arg, loc) in s.args.iter().zip(&locs) {
            match *loc {
                ArgumentLocation::Gpr(n) => self.emit_gpr_arg(arg, n)?,
                ArgumentLocation::Fpr(n) => self.emit_fpr_arg(arg, n)?,
                ArgumentLocation::Stack { .. } => {}
            }
        }

        let func_name = self.target.mangle_symbol(&s.link_name);
        self.insts.push(Instruction::Bl { func: func_name });

        if stack_bytes > 0 {
            self.insts.push(Instruction::AddSp { imm: stack_bytes });
        }

        self.insts.push(Instruction::RestoreCallerRegs);

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
            self.insts.push(return_inst(size, op));
        }
        self.insts.push(Instruction::Ret);
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
                self.insts.push(Instruction::Lea {
                    dst: Register::Virtual(new_ptr),
                    addr: FrameLayout::local_addr(slot),
                });
                self.insts.push(Instruction::Gep {
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
                self.insts.push(Instruction::Lea {
                    dst: Register::Virtual(new_ptr),
                    addr: Addr::Global(sym),
                });
                self.insts.push(Instruction::Gep {
                    dst: Register::Virtual(new_ptr),
                    base: Register::Virtual(new_ptr),
                    index,
                    scale: elem_size,
                });
            }
            (PtrBase::Register(base_v), _) => {
                self.insts.push(Instruction::Gep {
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
                self.insts.push(Instruction::Lea {
                    dst: Register::Virtual(dst),
                    addr: FrameLayout::local_addr_with_offset(slot, offset),
                });
            }
            (PtrBase::Stack, None) => {
                return Err(Error::Internal(
                    "missing stack slot for stack pointer".into(),
                ));
            }
            (PtrBase::Global(sym), _) => {
                self.insts.push(Instruction::Lea {
                    dst: Register::Virtual(dst),
                    addr: Addr::Global(sym),
                });
                if offset != 0 {
                    self.insts.push(Instruction::BinOp {
                        op: InstBinOp::Add,
                        size: RegisterSize::X64,
                        dst: Register::Virtual(dst),
                        lhs: Register::Virtual(dst),
                        rhs: InstOperand::Immediate(offset),
                    });
                }
            }
            (PtrBase::Register(base_v), _) => {
                self.insts.push(Instruction::Lea {
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

    /// Settles `arg` into the GPR-class argument slot `x{reg_idx}`
    /// (`w{reg_idx}` for 32-bit operands).  Pointer / array operands
    /// route through [`Self::emit_ptr_to_reg`] so an `alloca`-backed
    /// slot lowers to a frame-relative `lea`; scalar operands lower
    /// to a direct `mov` at the operand's natural width.
    fn emit_gpr_arg(&mut self, arg: &ir::Operand, reg_idx: u8) -> Result<(), Error> {
        let dst = Register::Physical(reg_idx);
        if matches!(
            arg.dtype(),
            ir::Dtype::Pointer { .. } | ir::Dtype::Array { .. }
        ) {
            self.emit_ptr_to_reg(arg, dst)
        } else {
            let (op, size) = self.lower_value(arg)?;
            self.insts.push(Instruction::Mov { size, dst, src: op });
            Ok(())
        }
    }

    /// Settles `arg` into the FPR-class argument slot `s{reg_idx}`
    /// via `fmov`.  Only reachable for floating-point operands; the
    /// pointer / array short-circuit in [`Self::emit_gpr_arg`] does
    /// not apply because no TeaLang pointer is ever AAPCS64-routed
    /// through the FPR file.
    fn emit_fpr_arg(&mut self, _arg: &ir::Operand, _reg_idx: u8) -> Result<(), Error> {
        todo!(
            "asmt-4: settle an f32 argument into the FPR slot `s{{reg_idx}}` via fmov; \
             see asmt-4.md §3.3"
        )
    }

    /// Writes `arg` into the caller's outgoing-arg area at
    /// `[sp, #stack_offset]`.  Immediate scalars are first
    /// materialised through `x16`/`w16`/`s16` because aarch64 has no
    /// store-immediate form; pointer / array operands materialise
    /// through `emit_ptr_to_reg` for the same `lea`-respecting reason
    /// as the GPR path.
    fn emit_stack_arg(&mut self, arg: &ir::Operand, stack_offset: i64) -> Result<(), Error> {
        let addr = outgoing_arg_addr(stack_offset);

        if matches!(
            arg.dtype(),
            ir::Dtype::Pointer { .. } | ir::Dtype::Array { .. }
        ) {
            let scratch = Register::Physical(REG_IP0);
            self.emit_ptr_to_reg(arg, scratch)?;
            self.insts.push(Instruction::Str {
                size: RegisterSize::X64,
                src: scratch,
                addr,
            });
            return Ok(());
        }

        let (op, size) = self.lower_value(arg)?;
        let src_reg = match op {
            InstOperand::Register(r) => r,
            InstOperand::Immediate(imm) => {
                let scratch = Register::Physical(REG_IP0);
                self.insts.push(Instruction::Mov {
                    size,
                    dst: scratch,
                    src: InstOperand::Immediate(imm),
                });
                scratch
            }
        };
        self.insts.push(Instruction::Str {
            size,
            src: src_reg,
            addr,
        });
        Ok(())
    }

    /// Lifts the AAPCS64 return value out of `x0` (or `s0` for FP
    /// returns) into the vreg named by `res`.  Dispatch is driven by
    /// the return dtype's [`RegisterSize`], so adding a new scalar
    /// class (e.g. `Dtype::F32 -> RegisterSize::S32`) requires only
    /// that `RegisterSize`'s `TryFrom<&ir::Dtype>` learns the new
    /// mapping.
    fn emit_call_result(&mut self, res: &ir::Local) -> Result<(), Error> {
        let dst = Register::Virtual(res.id.0);
        let size = RegisterSize::try_from(&res.dtype)?;
        self.insts.push(return_value_load(size, dst));
        Ok(())
    }

    fn emit_ptr_to_reg(&mut self, arg: &ir::Operand, dst: Register) -> Result<(), Error> {
        let (base_kind, slot) = self.lower_ptr(arg)?;
        match base_kind {
            PtrBase::Register(v) => {
                self.insts.push(Instruction::Mov {
                    size: RegisterSize::X64,
                    dst,
                    src: InstOperand::Register(Register::Virtual(v)),
                });
            }
            PtrBase::Stack => {
                let slot = slot.ok_or_else(|| Error::Internal("missing stack slot".into()))?;
                self.insts.push(Instruction::Lea {
                    dst,
                    addr: FrameLayout::local_addr(slot),
                });
            }
            PtrBase::Global(sym) => {
                self.insts.push(Instruction::Lea {
                    dst,
                    addr: Addr::Global(sym),
                });
            }
        }
        Ok(())
    }

    fn lower_int(&self, val: &ir::Operand) -> Result<InstOperand, Error> {
        match val {
            ir::Operand::Const(c) => Ok(InstOperand::Immediate(c.val)),
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
                Ok(InstOperand::Register(Register::Virtual(l.id.0)))
            }
            ir::Operand::Global(_) => Err(Error::UnsupportedOperand {
                what: format!("unsupported int operand: {}", val),
            }),
        }
    }

    fn lower_int_to_reg(&mut self, val: &ir::Operand) -> Result<Register, Error> {
        match self.lower_int(val)? {
            InstOperand::Register(r) => Ok(r),
            InstOperand::Immediate(imm) => {
                let tmp = self.fresh_vreg();
                self.insts.push(Instruction::Mov {
                    size: RegisterSize::W32,
                    dst: Register::Virtual(tmp),
                    src: InstOperand::Immediate(imm),
                });
                Ok(Register::Virtual(tmp))
            }
        }
    }

    fn lower_value(&self, val: &ir::Operand) -> Result<(InstOperand, RegisterSize), Error> {
        match val {
            ir::Operand::Const(c) => Ok((InstOperand::Immediate(c.val), RegisterSize::W32)),
            ir::Operand::Local(l) => {
                let size = match &l.dtype {
                    ir::Dtype::I1 | ir::Dtype::I32 => RegisterSize::W32,
                    ir::Dtype::Pointer { .. } => {
                        if self.frame.has_alloca(l.id.0) {
                            return Err(Error::UnsupportedOperand {
                                what: format!(
                                    "value operand uses alloca ptr %r{} directly (need address-of)",
                                    l.id.0
                                ),
                            });
                        }
                        RegisterSize::X64
                    }
                    other => {
                        return Err(Error::UnsupportedDtype {
                            dtype: other.clone(),
                        })
                    }
                };
                Ok((InstOperand::Register(Register::Virtual(l.id.0)), size))
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
                Ok(FrameLayout::local_addr(slot))
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
                // An alloca's address is its stack slot, not a value held
                // in a register.
                if let Some(slot) = self.frame.alloca_slot(vreg_index) {
                    return Ok((PtrBase::Stack, Some(slot)));
                }
                // A non-alloca pointer local holds its address in a virtual
                // register (e.g., the result of a GEP or load).
                if matches!(l.dtype, ir::Dtype::Pointer { .. }) {
                    return Ok((PtrBase::Register(vreg_index), None));
                }
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

    fn lower_index(&self, val: &ir::Operand) -> Result<InstOperand, Error> {
        match val {
            ir::Operand::Const(c) => Ok(InstOperand::Immediate(c.val)),
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
                Ok(InstOperand::Register(Register::Virtual(l.id.0)))
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
        let size = RegisterSize::try_from(dst.dtype())?;

        let src_op = match src {
            ir::Operand::Const(c) => InstOperand::Immediate(c.val),
            ir::Operand::Local(l) => InstOperand::Register(Register::Virtual(l.id.0)),
            ir::Operand::Global(_) => {
                return Err(Error::UnsupportedOperand {
                    what: "global variable in phi copy".into(),
                });
            }
        };

        let inst = match size {
            RegisterSize::W32 | RegisterSize::X64 => Instruction::Mov {
                size,
                dst: Register::Virtual(dst_vreg),
                src: src_op,
            },
            RegisterSize::S32 => todo!(
                "asmt-4: lower an f32 phi copy to `fmov` — `mov` is invalid \
                 between FP registers; see asmt-4.md §3.6"
            ),
        };
        self.insts.push(inst);
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

fn arith_op_to_binop(op: &ir::stmt::ArithBinOp) -> InstBinOp {
    match op {
        ir::stmt::ArithBinOp::Add => InstBinOp::Add,
        ir::stmt::ArithBinOp::Sub => InstBinOp::Sub,
        ir::stmt::ArithBinOp::Mul => InstBinOp::Mul,
        ir::stmt::ArithBinOp::SDiv => InstBinOp::SDiv,
    }
}

/// Builds the instruction that places the callee's return value into
/// its AAPCS64 register — `x0` (or `w0`) for integer/pointer returns
/// and `s0` for floating-point returns.  Selection is driven by
/// `size` so that adding a new scalar class only requires
/// `RegisterSize`'s `TryFrom<&ir::Dtype>` to learn the new mapping.
fn return_inst(size: RegisterSize, src: InstOperand) -> Instruction {
    match size {
        RegisterSize::W32 | RegisterSize::X64 => Instruction::Mov {
            size,
            dst: Register::Physical(REG_X0),
            src,
        },
        RegisterSize::S32 => todo!(
            "asmt-4: place an f32 return value into the FP return register `s0` \
             via fmov; see asmt-4.md §3.3"
        ),
    }
}

/// Dual of [`return_inst`] for the caller side: builds the
/// instruction that lifts the AAPCS64 return register into `dst`.
fn return_value_load(size: RegisterSize, dst: Register) -> Instruction {
    match size {
        RegisterSize::W32 | RegisterSize::X64 => Instruction::Mov {
            size,
            dst,
            src: InstOperand::Register(Register::Physical(REG_X0)),
        },
        RegisterSize::S32 => todo!(
            "asmt-4: lift an f32 call result out of the FP return register `s0` \
             into `dst` via fmov; see asmt-4.md §3.3"
        ),
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
