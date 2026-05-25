use std::io::Write;

use super::inst::Inst;
use super::types::{Addr, BinOp, Cond, IndexOperand, Operand, RegSize, Register};
use crate::asm::error::Error;
use crate::common::Target;

const SCRATCH0: u8 = 16;
const SCRATCH1: u8 = 17;

pub trait AsmPrint {
    fn emit_inst(&mut self, inst: &Inst) -> Result<(), Error>;

    fn emit_insts(&mut self, insts: &[Inst]) -> Result<(), Error> {
        for inst in insts {
            self.emit_inst(inst)?;
        }
        Ok(())
    }

    fn emit_sub_sp(&mut self, imm: i64) -> Result<(), Error>;

    fn emit_global(&mut self, sym: &str) -> Result<(), Error>;

    fn emit_align(&mut self, power: u32) -> Result<(), Error>;

    fn emit_label(&mut self, name: &str) -> Result<(), Error>;

    fn emit_prologue(&mut self, frame_size: i64) -> Result<(), Error>;

    fn emit_section(&mut self, name: &str) -> Result<(), Error>;

    fn emit_word(&mut self, value: i64) -> Result<(), Error>;

    fn emit_zero(&mut self, bytes: i64) -> Result<(), Error>;

    fn emit_newline(&mut self) -> Result<(), Error>;
}

pub struct AsmPrinter<W: Write> {
    writer: W,
    target: Target,
}

impl<W: Write> AsmPrinter<W> {
    pub fn new(writer: W, target: Target) -> Self {
        Self { writer, target }
    }

    fn reg_name(&self, r: Register, size: RegSize) -> String {
        match r {
            Register::StackPointer => "sp".to_string(),
            Register::Physical(31) => match size {
                RegSize::W32 => "wzr".to_string(),
                RegSize::X64 => "xzr".to_string(),
            },
            Register::Physical(n) => match size {
                RegSize::W32 => format!("w{n}"),
                RegSize::X64 => format!("x{n}"),
            },
            Register::Virtual(_) => {
                unreachable!("virtual regs should be eliminated before emission")
            }
        }
    }

    fn cond_suffix(&self, c: Cond) -> &'static str {
        match c {
            Cond::Eq => "eq",
            Cond::Ne => "ne",
            Cond::Lt => "lt",
            Cond::Le => "le",
            Cond::Gt => "gt",
            Cond::Ge => "ge",
        }
    }

    fn is_addsub_imm_encodable(&self, imm: i64) -> bool {
        if imm < 0 {
            return false;
        }
        if imm <= 4095 {
            return true;
        }
        imm % 4096 == 0 && (imm / 4096) <= 4095
    }

    fn is_ldr_str_offset_encodable(&self, size: RegSize, offset: i64) -> bool {
        if offset < 0 {
            return false;
        }
        let scale = match size {
            RegSize::W32 => 4,
            RegSize::X64 => 8,
        };
        offset % scale == 0 && (offset / scale) <= 4095
    }

    fn is_ldur_stur_offset_encodable(&self, offset: i64) -> bool {
        (-256..=255).contains(&offset)
    }

    fn scale_to_shift(&self, scale: i64) -> Option<u8> {
        match scale {
            1 => Some(0),
            2 => Some(1),
            4 => Some(2),
            8 => Some(3),
            _ => None,
        }
    }

    fn scratch_in_use(regs: &[Register], scratch: u8) -> bool {
        regs.iter()
            .any(|r| matches!(r, Register::Physical(n) if *n == scratch))
    }

    fn pick_scratch_reg(&self, regs: &[Register]) -> Option<Register> {
        let use0 = Self::scratch_in_use(regs, SCRATCH0);
        let use1 = Self::scratch_in_use(regs, SCRATCH1);
        match (use0, use1) {
            (false, _) => Some(Register::Physical(SCRATCH0)),
            (true, false) => Some(Register::Physical(SCRATCH1)),
            (true, true) => None,
        }
    }

    fn emit_mov(&mut self, size: RegSize, dst: Register, src: Operand) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, size);
        match src {
            Operand::Immediate(imm) => {
                writeln!(self.writer, "\tmov {dst_s}, #{imm}")?;
            }
            Operand::Register(r) => {
                let src_s = self.reg_name(r, size);
                writeln!(self.writer, "\tmov {dst_s}, {src_s}")?;
            }
        }
        Ok(())
    }

    fn emit_binop(
        &mut self,
        op: BinOp,
        size: RegSize,
        dst: Register,
        lhs: Register,
        rhs: Operand,
    ) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, size);
        let lhs_s = self.reg_name(lhs, size);

        match op {
            BinOp::Add | BinOp::Sub => match rhs {
                Operand::Immediate(imm) => {
                    let (op_mn, imm_abs) = match (op, imm < 0) {
                        (BinOp::Add, true) => ("sub", -imm),
                        (BinOp::Sub, true) => ("add", -imm),
                        (BinOp::Add, false) => ("add", imm),
                        (BinOp::Sub, false) => ("sub", imm),
                        _ => unreachable!(),
                    };
                    if self.is_addsub_imm_encodable(imm_abs) {
                        writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, #{imm_abs}")?;
                    } else if dst != lhs {
                        self.emit_mov_imm(&dst_s, imm_abs as u64)?;
                        writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, {dst_s}")?;
                    } else {
                        let scratch_reg = if matches!(dst, Register::Physical(r) if r == SCRATCH0) {
                            Register::Physical(SCRATCH1)
                        } else {
                            Register::Physical(SCRATCH0)
                        };
                        let scratch = self.reg_name(scratch_reg, size);
                        self.emit_mov_imm(&scratch, imm_abs as u64)?;
                        writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, {scratch}")?;
                    }
                }
                Operand::Register(r) => {
                    let rhs_s = self.reg_name(r, size);
                    let op_mn = match op {
                        BinOp::Add => "add",
                        BinOp::Sub => "sub",
                        _ => unreachable!(),
                    };
                    writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, {rhs_s}")?;
                }
            },
            BinOp::Mul => match rhs {
                Operand::Register(r) => {
                    let rhs_s = self.reg_name(r, size);
                    writeln!(self.writer, "\tmul {dst_s}, {lhs_s}, {rhs_s}")?;
                }
                Operand::Immediate(imm) => {
                    if dst != lhs {
                        self.emit_mov_imm(&dst_s, imm as u64)?;
                        writeln!(self.writer, "\tmul {dst_s}, {lhs_s}, {dst_s}")?;
                    } else {
                        let scratch_reg = if matches!(dst, Register::Physical(r) if r == SCRATCH0) {
                            Register::Physical(SCRATCH1)
                        } else {
                            Register::Physical(SCRATCH0)
                        };
                        let scratch = self.reg_name(scratch_reg, size);
                        self.emit_mov_imm(&scratch, imm as u64)?;
                        writeln!(self.writer, "\tmul {dst_s}, {lhs_s}, {scratch}")?;
                    }
                }
            },
            BinOp::SDiv => match rhs {
                Operand::Register(r) => {
                    let rhs_s = self.reg_name(r, size);
                    writeln!(self.writer, "\tsdiv {dst_s}, {lhs_s}, {rhs_s}")?;
                }
                Operand::Immediate(imm) => {
                    if dst != lhs {
                        self.emit_mov_imm(&dst_s, imm as u64)?;
                        writeln!(self.writer, "\tsdiv {dst_s}, {lhs_s}, {dst_s}")?;
                    } else {
                        let scratch_reg = if matches!(dst, Register::Physical(r) if r == SCRATCH0) {
                            Register::Physical(SCRATCH1)
                        } else {
                            Register::Physical(SCRATCH0)
                        };
                        let scratch = self.reg_name(scratch_reg, size);
                        self.emit_mov_imm(&scratch, imm as u64)?;
                        writeln!(self.writer, "\tsdiv {dst_s}, {lhs_s}, {scratch}")?;
                    }
                }
            },
        }

        Ok(())
    }

    fn emit_load(&mut self, size: RegSize, dst: Register, addr: &Addr) -> Result<(), Error> {
        self.emit_mem_access("ldr", size, dst, addr)
    }

    fn emit_store(&mut self, size: RegSize, src: Register, addr: &Addr) -> Result<(), Error> {
        self.emit_mem_access("str", size, src, addr)
    }

    fn emit_mem_access(
        &mut self,
        mnemonic: &str,
        size: RegSize,
        reg: Register,
        addr: &Addr,
    ) -> Result<(), Error> {
        let reg_s = self.reg_name(reg, size);

        match addr {
            Addr::BaseOff { base, offset } => {
                let base_s = self.reg_name(*base, RegSize::X64);
                if *offset == 0 {
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{base_s}]")?;
                } else if self.is_ldr_str_offset_encodable(size, *offset) {
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{base_s}, #{offset}]")?;
                } else if self.is_ldur_stur_offset_encodable(*offset) {
                    let unscaled = if mnemonic == "ldr" { "ldur" } else { "stur" };
                    writeln!(self.writer, "\t{unscaled} {reg_s}, [{base_s}, #{offset}]")?;
                } else if mnemonic == "ldr" {
                    let addr_s = self.reg_name(reg, RegSize::X64);
                    let (op_mn, imm_abs) = if *offset < 0 {
                        ("sub", -offset)
                    } else {
                        ("add", *offset)
                    };
                    self.emit_mov_imm(&addr_s, imm_abs as u64)?;
                    writeln!(self.writer, "\t{op_mn} {addr_s}, {base_s}, {addr_s}")?;
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{addr_s}]")?;
                } else {
                    let scratch = self.pick_scratch_reg(&[reg, *base]);
                    if let Some(scratch) = scratch {
                        let scratch_s = self.reg_name(scratch, RegSize::X64);
                        self.emit_add_x_imm_with(scratch, *base, *offset, scratch)?;
                        writeln!(self.writer, "\t{mnemonic} {reg_s}, [{scratch_s}]")?;
                    } else {
                        self.emit_sub_sp(16)?;
                        writeln!(self.writer, "\tstr {reg_s}, [sp]")?;

                        let addr_s = self.reg_name(reg, RegSize::X64);
                        let (op_mn, imm_abs) = if *offset < 0 {
                            ("sub", -offset)
                        } else {
                            ("add", *offset)
                        };
                        self.emit_mov_imm(&addr_s, imm_abs as u64)?;
                        writeln!(self.writer, "\t{op_mn} {addr_s}, {base_s}, {addr_s}")?;

                        let restored_s = self.reg_name(*base, size);
                        writeln!(self.writer, "\tldr {restored_s}, [sp]")?;
                        self.emit_add_sp(16)?;
                        writeln!(self.writer, "\t{mnemonic} {restored_s}, [{addr_s}]")?;
                    }
                }
            }
            Addr::Global(sym) => {
                if mnemonic == "ldr" {
                    let addr_s = self.reg_name(reg, RegSize::X64);
                    self.emit_adrp_add(reg, sym)?;
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{addr_s}]")?;
                } else {
                    let scratch = self
                        .pick_scratch_reg(&[reg])
                        .unwrap_or(Register::Physical(SCRATCH0));
                    let scratch_s = self.reg_name(scratch, RegSize::X64);
                    self.emit_adrp_add(scratch, sym)?;
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{scratch_s}]")?;
                }
            }
        }
        Ok(())
    }

    fn emit_lea(&mut self, dst: Register, addr: &Addr) -> Result<(), Error> {
        match addr {
            Addr::Global(sym) => self.emit_adrp_add(dst, sym),
            Addr::BaseOff { base, offset: 0 } => {
                writeln!(
                    self.writer,
                    "\tmov {}, {}",
                    self.reg_name(dst, RegSize::X64),
                    self.reg_name(*base, RegSize::X64)
                )?;
                Ok(())
            }
            Addr::BaseOff { base, offset } => {
                let scratch = self.pick_scratch_reg(&[dst, *base]).unwrap_or(dst);
                self.emit_add_x_imm_with(dst, *base, *offset, scratch)
            }
        }
    }

    fn emit_gep(
        &mut self,
        dst: Register,
        base: Register,
        index: IndexOperand,
        scale: i64,
    ) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, RegSize::X64);
        let base_s = self.reg_name(base, RegSize::X64);

        match index {
            IndexOperand::Imm(i) => {
                let off = i * scale;
                if off == 0 {
                    writeln!(self.writer, "\tmov {dst_s}, {base_s}")?;
                } else {
                    let scratch = self.pick_scratch_reg(&[dst, base]).unwrap_or(dst);
                    self.emit_add_x_imm_with(dst, base, off, scratch)?;
                }
            }
            IndexOperand::Reg(r) => {
                let idx_s = self.reg_name(r, RegSize::W32);

                if let Some(shift) = self.scale_to_shift(scale) {
                    writeln!(
                        self.writer,
                        "\tadd {dst_s}, {base_s}, {idx_s}, sxtw #{shift}"
                    )?;
                } else if dst != base {
                    let tmp0_s = self.reg_name(dst, RegSize::X64);
                    if let Some(tmp1) = self.pick_scratch_reg(&[dst, base]) {
                        let tmp1_s = self.reg_name(tmp1, RegSize::X64);
                        writeln!(self.writer, "\tsxtw {tmp0_s}, {idx_s}")?;
                        self.emit_mov_imm(&tmp1_s, scale as u64)?;
                        writeln!(self.writer, "\tmul {tmp0_s}, {tmp0_s}, {tmp1_s}")?;
                        writeln!(self.writer, "\tadd {dst_s}, {base_s}, {tmp0_s}")?;
                    } else {
                        // {dst, base} occupies both scratch registers, leaving
                        // no third register for the scale immediate.  Stash
                        // base on the stack so its register can hold the
                        // scale, then reload base for the final add.
                        self.emit_sub_sp(16)?;
                        writeln!(self.writer, "\tstr {base_s}, [sp]")?;
                        writeln!(self.writer, "\tsxtw {tmp0_s}, {idx_s}")?;
                        self.emit_mov_imm(&base_s, scale as u64)?;
                        writeln!(self.writer, "\tmul {tmp0_s}, {tmp0_s}, {base_s}")?;
                        writeln!(self.writer, "\tldr {base_s}, [sp]")?;
                        self.emit_add_sp(16)?;
                        writeln!(self.writer, "\tadd {dst_s}, {base_s}, {tmp0_s}")?;
                    }
                } else if matches!(base, Register::Physical(r) if r == SCRATCH0)
                    || matches!(base, Register::Physical(r) if r == SCRATCH1)
                {
                    let other = if matches!(base, Register::Physical(r) if r == SCRATCH0) {
                        Register::Physical(SCRATCH1)
                    } else {
                        Register::Physical(SCRATCH0)
                    };
                    let tmp0_s = self.reg_name(base, RegSize::X64);
                    let tmp1_s = self.reg_name(other, RegSize::X64);
                    self.emit_sub_sp(16)?;
                    writeln!(self.writer, "\tstr {tmp0_s}, [sp]")?;
                    writeln!(self.writer, "\tsxtw {tmp0_s}, {idx_s}")?;
                    self.emit_mov_imm(&tmp1_s, scale as u64)?;
                    writeln!(self.writer, "\tmul {tmp0_s}, {tmp0_s}, {tmp1_s}")?;
                    writeln!(self.writer, "\tldr {tmp1_s}, [sp]")?;
                    self.emit_add_sp(16)?;
                    writeln!(self.writer, "\tadd {dst_s}, {tmp1_s}, {tmp0_s}")?;
                } else {
                    let scratch0 = self.reg_name(Register::Physical(SCRATCH0), RegSize::X64);
                    let scratch1 = self.reg_name(Register::Physical(SCRATCH1), RegSize::X64);
                    writeln!(self.writer, "\tsxtw {scratch0}, {idx_s}")?;
                    self.emit_mov_imm(&scratch1, scale as u64)?;
                    writeln!(self.writer, "\tmul {scratch0}, {scratch0}, {scratch1}")?;
                    writeln!(self.writer, "\tadd {dst_s}, {base_s}, {scratch0}")?;
                }
            }
        }
        Ok(())
    }

    fn emit_cmp(&mut self, size: RegSize, lhs: Register, rhs: Operand) -> Result<(), Error> {
        let lhs_s = self.reg_name(lhs, size);
        match rhs {
            Operand::Register(r) => {
                writeln!(self.writer, "\tcmp {lhs_s}, {}", self.reg_name(r, size))?
            }
            Operand::Immediate(imm) if self.is_addsub_imm_encodable(imm) => {
                writeln!(self.writer, "\tcmp {lhs_s}, #{imm}")?
            }
            Operand::Immediate(imm) => {
                let scratch_reg = if matches!(lhs, Register::Physical(r) if r == SCRATCH0) {
                    Register::Physical(SCRATCH1)
                } else {
                    Register::Physical(SCRATCH0)
                };
                let scratch = self.reg_name(scratch_reg, size);
                self.emit_mov_imm(&scratch, imm as u64)?;
                writeln!(self.writer, "\tcmp {lhs_s}, {scratch}")?;
            }
        }
        Ok(())
    }

    fn emit_adrp_add(&mut self, dst: Register, sym: &str) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, RegSize::X64);
        match self.target {
            Target::Macos => {
                writeln!(self.writer, "\tadrp {dst_s}, {sym}@PAGE")?;
                writeln!(self.writer, "\tadd  {dst_s}, {dst_s}, {sym}@PAGEOFF")?;
            }
            Target::Linux => {
                writeln!(self.writer, "\tadrp {dst_s}, {sym}")?;
                writeln!(self.writer, "\tadd  {dst_s}, {dst_s}, :lo12:{sym}")?;
            }
        }
        Ok(())
    }

    fn emit_mov_imm(&mut self, reg: &str, value: u64) -> Result<(), Error> {
        let is_32bit = reg.starts_with('w');
        let value = if is_32bit { value & 0xFFFF_FFFF } else { value };

        if value <= 0xFFFF {
            writeln!(self.writer, "\tmov {reg}, #{value}")?;
            return Ok(());
        }

        let chunk0 = (value & 0xFFFF) as u16;
        let chunk1 = ((value >> 16) & 0xFFFF) as u16;
        let chunk2 = ((value >> 32) & 0xFFFF) as u16;
        let chunk3 = ((value >> 48) & 0xFFFF) as u16;

        // Find the first non-zero chunk to use movz
        let mut first = true;
        if chunk0 != 0 || (chunk1 == 0 && chunk2 == 0 && chunk3 == 0) {
            writeln!(self.writer, "\tmovz {reg}, #{chunk0}")?;
            first = false;
        }
        if chunk1 != 0 {
            if first {
                writeln!(self.writer, "\tmovz {reg}, #{chunk1}, lsl #16")?;
                first = false;
            } else {
                writeln!(self.writer, "\tmovk {reg}, #{chunk1}, lsl #16")?;
            }
        }
        if !is_32bit {
            if chunk2 != 0 {
                if first {
                    writeln!(self.writer, "\tmovz {reg}, #{chunk2}, lsl #32")?;
                    first = false;
                } else {
                    writeln!(self.writer, "\tmovk {reg}, #{chunk2}, lsl #32")?;
                }
            }
            if chunk3 != 0 {
                if first {
                    writeln!(self.writer, "\tmovz {reg}, #{chunk3}, lsl #48")?;
                } else {
                    writeln!(self.writer, "\tmovk {reg}, #{chunk3}, lsl #48")?;
                }
            }
        }

        Ok(())
    }

    fn emit_add_x_imm_with(
        &mut self,
        dst: Register,
        base: Register,
        offset: i64,
        scratch: Register,
    ) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, RegSize::X64);
        let base_s = self.reg_name(base, RegSize::X64);

        match offset {
            0 => writeln!(self.writer, "\tmov {dst_s}, {base_s}")?,
            off if off > 0 && self.is_addsub_imm_encodable(off) => {
                writeln!(self.writer, "\tadd {dst_s}, {base_s}, #{off}")?
            }
            off if off < 0 && self.is_addsub_imm_encodable(-off) => {
                writeln!(self.writer, "\tsub {dst_s}, {base_s}, #{}", -off)?
            }
            off => {
                let scratch_s = self.reg_name(scratch, RegSize::X64);
                self.emit_mov_imm(&scratch_s, off.unsigned_abs())?;
                if off > 0 {
                    writeln!(self.writer, "\tadd {dst_s}, {base_s}, {scratch_s}")?;
                } else {
                    writeln!(self.writer, "\tsub {dst_s}, {base_s}, {scratch_s}")?;
                }
            }
        }
        Ok(())
    }

    fn emit_add_sp(&mut self, imm: i64) -> Result<(), Error> {
        if imm == 0 {
            return Ok(());
        }
        if self.is_addsub_imm_encodable(imm) {
            writeln!(self.writer, "\tadd sp, sp, #{imm}")?;
        } else {
            let scratch = self.reg_name(Register::Physical(SCRATCH0), RegSize::X64);
            self.emit_mov_imm(&scratch, imm as u64)?;
            writeln!(self.writer, "\tadd sp, sp, {scratch}")?;
        }
        Ok(())
    }

    fn emit_save_caller_regs(&mut self) -> Result<(), Error> {
        writeln!(self.writer, "\tstr x15, [sp, #-16]!")?;
        writeln!(self.writer, "\tstp x13, x14, [sp, #-16]!")?;
        writeln!(self.writer, "\tstp x11, x12, [sp, #-16]!")?;
        writeln!(self.writer, "\tstp x9,  x10, [sp, #-16]!")?;
        writeln!(self.writer, "\tstr x8,  [sp, #-16]!")?;
        Ok(())
    }

    fn emit_restore_caller_regs(&mut self) -> Result<(), Error> {
        writeln!(self.writer, "\tldr x8,  [sp], #16")?;
        writeln!(self.writer, "\tldp x9,  x10, [sp], #16")?;
        writeln!(self.writer, "\tldp x11, x12, [sp], #16")?;
        writeln!(self.writer, "\tldp x13, x14, [sp], #16")?;
        writeln!(self.writer, "\tldr x15, [sp], #16")?;
        Ok(())
    }
}

impl<W: Write> AsmPrint for AsmPrinter<W> {
    fn emit_inst(&mut self, inst: &Inst) -> Result<(), Error> {
        match inst {
            Inst::Label(name) => writeln!(self.writer, "{name}:")?,
            Inst::Mov { size, dst, src } => self.emit_mov(*size, *dst, *src)?,
            Inst::BinOp {
                op,
                size,
                dst,
                lhs,
                rhs,
            } => self.emit_binop(*op, *size, *dst, *lhs, *rhs)?,
            Inst::Ldr { size, dst, addr } => self.emit_load(*size, *dst, addr)?,
            Inst::Str { size, src, addr } => self.emit_store(*size, *src, addr)?,
            Inst::Lea { dst, addr } => self.emit_lea(*dst, addr)?,
            Inst::Gep {
                dst,
                base,
                index,
                scale,
            } => self.emit_gep(*dst, *base, *index, *scale)?,
            Inst::Cmp { size, lhs, rhs } => self.emit_cmp(*size, *lhs, *rhs)?,
            Inst::B { label } => writeln!(self.writer, "\tb {label}")?,
            Inst::BCond { cond, label } => {
                writeln!(self.writer, "\tb.{} {label}", self.cond_suffix(*cond))?
            }
            Inst::Bl { func } => writeln!(self.writer, "\tbl {func}")?,
            Inst::SaveCallerRegs => self.emit_save_caller_regs()?,
            Inst::RestoreCallerRegs => self.emit_restore_caller_regs()?,
            Inst::SubSp { imm } => self.emit_sub_sp(*imm)?,
            Inst::AddSp { imm } => self.emit_add_sp(*imm)?,
            Inst::Ret => {
                writeln!(self.writer, "\tmov sp, x29")?;
                writeln!(self.writer, "\tldp x29, x30, [sp], #16")?;
                writeln!(self.writer, "\tret")?;
            }
        }
        Ok(())
    }

    fn emit_sub_sp(&mut self, imm: i64) -> Result<(), Error> {
        if imm == 0 {
            return Ok(());
        }
        if self.is_addsub_imm_encodable(imm) {
            writeln!(self.writer, "\tsub sp, sp, #{imm}")?;
        } else {
            let scratch = self.reg_name(Register::Physical(SCRATCH0), RegSize::X64);
            self.emit_mov_imm(&scratch, imm as u64)?;
            writeln!(self.writer, "\tsub sp, sp, {scratch}")?;
        }
        Ok(())
    }

    fn emit_global(&mut self, sym: &str) -> Result<(), Error> {
        writeln!(self.writer, ".globl {sym}")?;
        Ok(())
    }

    fn emit_align(&mut self, power: u32) -> Result<(), Error> {
        writeln!(self.writer, ".p2align {power}")?;
        Ok(())
    }

    fn emit_label(&mut self, name: &str) -> Result<(), Error> {
        writeln!(self.writer, "{name}:")?;
        Ok(())
    }

    fn emit_prologue(&mut self, frame_size: i64) -> Result<(), Error> {
        writeln!(self.writer, "\tstp x29, x30, [sp, #-16]!")?;
        writeln!(self.writer, "\tmov x29, sp")?;
        if frame_size > 0 {
            self.emit_sub_sp(frame_size)?;
        }
        Ok(())
    }

    fn emit_section(&mut self, name: &str) -> Result<(), Error> {
        writeln!(self.writer, ".{name}")?;
        Ok(())
    }

    fn emit_word(&mut self, value: i64) -> Result<(), Error> {
        writeln!(self.writer, "\t.word {value}")?;
        Ok(())
    }

    fn emit_zero(&mut self, bytes: i64) -> Result<(), Error> {
        writeln!(self.writer, "\t.zero {bytes}")?;
        Ok(())
    }

    fn emit_newline(&mut self) -> Result<(), Error> {
        writeln!(self.writer)?;
        Ok(())
    }
}
