//! Textual emitter for the finalized instruction stream: resolves
//! register names and immediate encodings (falling back to scratch
//! registers where an operand does not fit), and orchestrates
//! whole-program assembly output — sections, globals, and functions.

use std::io::Write;

use super::frame::SCRATCH_SPILL_SLOT;
use super::inst::Instruction;
use super::types::{
    Addr, Cond, FBinOp, InstBinOp, InstOperand, RegisterSize, Register, SCRATCH0, SCRATCH1,
};
use super::{GeneratedFunction, GeneratedGlobal, GlobalData};
use crate::asm::error::Error;
use crate::common::Target;

pub struct AsmPrinter<W: Write> {
    writer: W,
    target: Target,
    /// Whether the function currently being emitted uses any FP
    /// register.  Gates the floating-point half of the caller-saved
    /// register bracket so integer-only functions emit no FP spill
    /// code around their calls.
    current_fn_uses_fp: bool,
}

impl<W: Write> AsmPrinter<W> {
    pub fn new(writer: W, target: Target) -> Self {
        Self {
            writer,
            target,
            current_fn_uses_fp: false,
        }
    }

    /// Records whether the function about to be emitted uses FP, so the
    /// caller-saved bracket knows whether to preserve the FP pool
    /// (`d18`–`d25`) around its calls.
    pub fn set_uses_fp(&mut self, uses_fp: bool) {
        self.current_fn_uses_fp = uses_fp;
    }

    /// Emits the complete assembly file: a `.data` section holding
    /// every global (when any exist), then the `.text` section with
    /// every function.  The single source of truth for what a teac `.s`
    /// file looks like; `AArch64AsmGenerator::output` delegates here
    /// wholesale, mirroring how the ir layer's `output` delegates to
    /// [`crate::ir::printer::IrPrinter::emit_module`].
    pub fn emit_program(
        &mut self,
        globals: &[GeneratedGlobal],
        functions: &[GeneratedFunction],
    ) -> Result<(), Error> {
        if !globals.is_empty() {
            self.emit_section("data")?;
            for g in globals {
                self.emit_global(&g.symbol)?;
                self.emit_align(2)?;
                self.emit_label(&g.symbol)?;
                match &g.data {
                    GlobalData::Word { value } => self.emit_word(*value)?,
                    GlobalData::Array { words, zero_bytes } => {
                        for v in words {
                            self.emit_word(*v)?;
                        }
                        if *zero_bytes > 0 {
                            self.emit_zero(*zero_bytes)?;
                        }
                    }
                }
            }
            self.emit_newline()?;
        }

        self.emit_section("text")?;
        for func in functions {
            self.emit_function(func)?;
        }

        Ok(())
    }

    /// Emits one function: symbol directive, alignment, label, the
    /// FP-usage bracket flag, prologue, the finalized instruction
    /// stream, and a trailing blank line.
    pub fn emit_function(&mut self, func: &GeneratedFunction) -> Result<(), Error> {
        self.emit_global(&func.symbol)?;
        self.emit_align(2)?;
        self.emit_label(&func.symbol)?;
        self.set_uses_fp(func.uses_fp);
        self.emit_prologue(func.frame_size)?;
        self.emit_insts(&func.insts)?;
        self.emit_newline()
    }

    fn reg_name(&self, r: Register, size: RegisterSize) -> String {
        match r {
            Register::StackPointer => "sp".to_string(),
            Register::Physical(31) => match size {
                RegisterSize::W32 => "wzr".to_string(),
                RegisterSize::X64 => "xzr".to_string(),
                RegisterSize::S32 => unreachable!("zero register has no FP form"),
            },
            Register::Physical(n) => match size {
                RegisterSize::W32 => format!("w{n}"),
                RegisterSize::X64 => format!("x{n}"),
                RegisterSize::S32 => format!("s{n}"),
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

    fn is_ldr_str_offset_encodable(&self, size: RegisterSize, offset: i64) -> bool {
        if offset < 0 {
            return false;
        }
        let scale = match size {
            RegisterSize::W32 | RegisterSize::S32 => 4,
            RegisterSize::X64 => 8,
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

    /// Returns a scratch register that is **not** present in `regs`, or
    /// `None` when both `SCRATCH0` and `SCRATCH1` already appear in `regs`.
    ///
    /// Callers that pass at most one register in `regs` are guaranteed at
    /// least one free scratch; callers that pass two registers should
    /// handle the `None` case explicitly (typically via a stack-spill
    /// path or [`pick_scratch_or_clobber_dst`]).
    fn pick_free_scratch(&self, regs: &[Register]) -> Option<Register> {
        let use0 = Self::scratch_in_use(regs, SCRATCH0);
        let use1 = Self::scratch_in_use(regs, SCRATCH1);
        match (use0, use1) {
            (false, _) => Some(Register::Physical(SCRATCH0)),
            (true, false) => Some(Register::Physical(SCRATCH1)),
            (true, true) => None,
        }
    }

    /// Like [`pick_free_scratch`] but falls back to `dst` when both
    /// scratches are taken.
    ///
    /// The fallback is sound **only** for the
    /// `mov scratch, #imm; add dst, base, scratch` sequence emitted by
    /// [`emit_add_x_imm_with`]: when `scratch == dst`, the initial `mov`
    /// overwrites `dst` with `#imm`, and the subsequent `add dst, base,
    /// dst` still computes `base + imm` correctly because `dst` carries
    /// `imm` at that point.  Using this helper for any other op
    /// sequence is unsound — the identity does not generalise.
    fn pick_scratch_or_clobber_dst(&self, regs: &[Register], dst: Register) -> Register {
        self.pick_free_scratch(regs).unwrap_or(dst)
    }

    fn emit_mov(&mut self, size: RegisterSize, dst: Register, src: InstOperand) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, size);
        match src {
            InstOperand::Immediate(imm) => {
                writeln!(self.writer, "\tmov {dst_s}, #{imm}")?;
            }
            InstOperand::Register(r) => {
                let src_s = self.reg_name(r, size);
                writeln!(self.writer, "\tmov {dst_s}, {src_s}")?;
            }
        }
        Ok(())
    }

    fn emit_binop(
        &mut self,
        op: InstBinOp,
        size: RegisterSize,
        dst: Register,
        lhs: Register,
        rhs: InstOperand,
    ) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, size);
        let lhs_s = self.reg_name(lhs, size);

        match (op, rhs) {
            (InstBinOp::Add | InstBinOp::Sub, InstOperand::Immediate(imm)) => {
                let (op_mn, imm_abs) = match (op, imm < 0) {
                    (InstBinOp::Add, true) => ("sub", -imm),
                    (InstBinOp::Sub, true) => ("add", -imm),
                    (InstBinOp::Add, false) => ("add", imm),
                    (InstBinOp::Sub, false) => ("sub", imm),
                    _ => unreachable!(),
                };
                if self.is_addsub_imm_encodable(imm_abs) {
                    writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, #{imm_abs}")?;
                } else {
                    self.emit_op_via_imm_scratch(op_mn, size, dst, lhs, imm_abs as u64)?;
                }
            }
            (InstBinOp::Add, InstOperand::Register(r)) => {
                let rhs_s = self.reg_name(r, size);
                writeln!(self.writer, "\tadd {dst_s}, {lhs_s}, {rhs_s}")?;
            }
            (InstBinOp::Sub, InstOperand::Register(r)) => {
                let rhs_s = self.reg_name(r, size);
                writeln!(self.writer, "\tsub {dst_s}, {lhs_s}, {rhs_s}")?;
            }
            (InstBinOp::Mul, InstOperand::Register(r)) => {
                let rhs_s = self.reg_name(r, size);
                writeln!(self.writer, "\tmul {dst_s}, {lhs_s}, {rhs_s}")?;
            }
            (InstBinOp::Mul, InstOperand::Immediate(imm)) => {
                self.emit_op_via_imm_scratch("mul", size, dst, lhs, imm as u64)?;
            }
            (InstBinOp::SDiv, InstOperand::Register(r)) => {
                let rhs_s = self.reg_name(r, size);
                writeln!(self.writer, "\tsdiv {dst_s}, {lhs_s}, {rhs_s}")?;
            }
            (InstBinOp::SDiv, InstOperand::Immediate(imm)) => {
                self.emit_op_via_imm_scratch("sdiv", size, dst, lhs, imm as u64)?;
            }
        }

        Ok(())
    }

    /// Emits the `mov scratch, #imm; op_mn dst, lhs, scratch` sequence
    /// used for any binary op with an immediate `rhs` that cannot be
    /// encoded inline.  When `dst != lhs`, the destination register
    /// doubles as the scratch (safe because `mov dst, #imm` runs before
    /// the op reads `dst`).  When `dst == lhs`, the source must be
    /// preserved, so `scratch` is the *other* of `SCRATCH0`/`SCRATCH1`.
    fn emit_op_via_imm_scratch(
        &mut self,
        op_mn: &str,
        size: RegisterSize,
        dst: Register,
        lhs: Register,
        imm: u64,
    ) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, size);
        let lhs_s = self.reg_name(lhs, size);
        if dst != lhs {
            self.emit_mov_imm(&dst_s, imm)?;
            writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, {dst_s}")?;
        } else {
            let scratch_reg = if matches!(dst, Register::Physical(r) if r == SCRATCH0) {
                Register::Physical(SCRATCH1)
            } else {
                Register::Physical(SCRATCH0)
            };
            let scratch = self.reg_name(scratch_reg, size);
            self.emit_mov_imm(&scratch, imm)?;
            writeln!(self.writer, "\t{op_mn} {dst_s}, {lhs_s}, {scratch}")?;
        }
        Ok(())
    }

    fn emit_load(&mut self, size: RegisterSize, dst: Register, addr: &Addr) -> Result<(), Error> {
        self.emit_mem_access("ldr", size, dst, addr)
    }

    fn emit_store(&mut self, size: RegisterSize, src: Register, addr: &Addr) -> Result<(), Error> {
        self.emit_mem_access("str", size, src, addr)
    }

    fn emit_mem_access(
        &mut self,
        mnemonic: &str,
        size: RegisterSize,
        reg: Register,
        addr: &Addr,
    ) -> Result<(), Error> {
        let reg_s = self.reg_name(reg, size);

        match addr {
            Addr::BaseOff { base, offset } => {
                let base_s = self.reg_name(*base, RegisterSize::X64);
                if *offset == 0 {
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{base_s}]")?;
                } else if self.is_ldr_str_offset_encodable(size, *offset) {
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{base_s}, #{offset}]")?;
                } else if self.is_ldur_stur_offset_encodable(*offset) {
                    let unscaled = if mnemonic == "ldr" { "ldur" } else { "stur" };
                    writeln!(self.writer, "\t{unscaled} {reg_s}, [{base_s}, #{offset}]")?;
                } else if mnemonic == "ldr" {
                    let addr_s = self.reg_name(reg, RegisterSize::X64);
                    let (op_mn, imm_abs) = if *offset < 0 {
                        ("sub", -offset)
                    } else {
                        ("add", *offset)
                    };
                    self.emit_mov_imm(&addr_s, imm_abs as u64)?;
                    writeln!(self.writer, "\t{op_mn} {addr_s}, {base_s}, {addr_s}")?;
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{addr_s}]")?;
                } else if let Some(scratch) = self.pick_free_scratch(&[reg, *base]) {
                    let scratch_s = self.reg_name(scratch, RegisterSize::X64);
                    self.emit_add_x_imm_with(scratch, *base, *offset, scratch)?;
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{scratch_s}]")?;
                } else {
                    // `str` with a non-encodable offset needs a temporary
                    // to hold `base + offset`.  Here `reg` and `base`
                    // already occupy both `SCRATCH0` and `SCRATCH1`, so
                    // neither can serve as the temporary.  AArch64
                    // intra-procedure-call scratch is limited to two
                    // registers (x16/x17) by ABI; there is no third
                    // GPR scratch to reach for.  Bounce `reg` through
                    // the stack, repurpose `reg` to compute the address,
                    // then restore `reg`'s value into `base` (which is
                    // free at that point since the original `reg` lives
                    // in `[sp]`).
                    self.emit_sub_sp(SCRATCH_SPILL_SLOT)?;
                    writeln!(self.writer, "\tstr {reg_s}, [sp]")?;

                    let addr_s = self.reg_name(reg, RegisterSize::X64);
                    let (op_mn, imm_abs) = if *offset < 0 {
                        ("sub", -offset)
                    } else {
                        ("add", *offset)
                    };
                    self.emit_mov_imm(&addr_s, imm_abs as u64)?;
                    writeln!(self.writer, "\t{op_mn} {addr_s}, {base_s}, {addr_s}")?;

                    let restored_s = self.reg_name(*base, size);
                    writeln!(self.writer, "\tldr {restored_s}, [sp]")?;
                    self.emit_add_sp(SCRATCH_SPILL_SLOT)?;
                    writeln!(self.writer, "\t{mnemonic} {restored_s}, [{addr_s}]")?;
                }
            }
            Addr::Global(sym) => {
                if mnemonic == "ldr" {
                    let addr_s = self.reg_name(reg, RegisterSize::X64);
                    self.emit_adrp_add(reg, sym)?;
                    writeln!(self.writer, "\t{mnemonic} {reg_s}, [{addr_s}]")?;
                } else {
                    let scratch = self
                        .pick_free_scratch(&[reg])
                        .expect("single-register input always leaves at least one scratch free");
                    let scratch_s = self.reg_name(scratch, RegisterSize::X64);
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
                    self.reg_name(dst, RegisterSize::X64),
                    self.reg_name(*base, RegisterSize::X64)
                )?;
                Ok(())
            }
            Addr::BaseOff { base, offset } => {
                let scratch = self.pick_scratch_or_clobber_dst(&[dst, *base], dst);
                self.emit_add_x_imm_with(dst, *base, *offset, scratch)
            }
        }
    }

    fn emit_gep(
        &mut self,
        dst: Register,
        base: Register,
        index: InstOperand,
        scale: i64,
    ) -> Result<(), Error> {
        let dst_s = self.reg_name(dst, RegisterSize::X64);
        let base_s = self.reg_name(base, RegisterSize::X64);

        match index {
            InstOperand::Immediate(i) => {
                let off = i * scale;
                if off == 0 {
                    writeln!(self.writer, "\tmov {dst_s}, {base_s}")?;
                } else {
                    let scratch = self.pick_scratch_or_clobber_dst(&[dst, base], dst);
                    self.emit_add_x_imm_with(dst, base, off, scratch)?;
                }
            }
            InstOperand::Register(r) => {
                let idx_s = self.reg_name(r, RegisterSize::W32);

                if let Some(shift) = self.scale_to_shift(scale) {
                    writeln!(
                        self.writer,
                        "\tadd {dst_s}, {base_s}, {idx_s}, sxtw #{shift}"
                    )?;
                } else if dst != base {
                    let tmp0_s = self.reg_name(dst, RegisterSize::X64);
                    if let Some(tmp1) = self.pick_free_scratch(&[dst, base]) {
                        let tmp1_s = self.reg_name(tmp1, RegisterSize::X64);
                        writeln!(self.writer, "\tsxtw {tmp0_s}, {idx_s}")?;
                        self.emit_mov_imm(&tmp1_s, scale as u64)?;
                        writeln!(self.writer, "\tmul {tmp0_s}, {tmp0_s}, {tmp1_s}")?;
                        writeln!(self.writer, "\tadd {dst_s}, {base_s}, {tmp0_s}")?;
                    } else {
                        // `dst` and `base` together occupy both `SCRATCH0`
                        // and `SCRATCH1`; no third GPR scratch exists in
                        // AAPCS64 (x16/x17 are the only ABI IP registers,
                        // x18 is platform-reserved on macOS, x19+ are
                        // callee-saved).  Stash `base` on the stack so its
                        // register can hold the scale, multiply into
                        // `tmp0` (== `dst`), reload `base`, then add.
                        self.emit_sub_sp(SCRATCH_SPILL_SLOT)?;
                        writeln!(self.writer, "\tstr {base_s}, [sp]")?;
                        writeln!(self.writer, "\tsxtw {tmp0_s}, {idx_s}")?;
                        self.emit_mov_imm(&base_s, scale as u64)?;
                        writeln!(self.writer, "\tmul {tmp0_s}, {tmp0_s}, {base_s}")?;
                        writeln!(self.writer, "\tldr {base_s}, [sp]")?;
                        self.emit_add_sp(SCRATCH_SPILL_SLOT)?;
                        writeln!(self.writer, "\tadd {dst_s}, {base_s}, {tmp0_s}")?;
                    }
                } else if matches!(base, Register::Physical(r) if r == SCRATCH0)
                    || matches!(base, Register::Physical(r) if r == SCRATCH1)
                {
                    // `dst == base` and the shared register lives in one
                    // of the scratch slots.  The other scratch is free,
                    // but a third register is still required for the
                    // partial product; bounce the original `base` value
                    // through the stack so its register can be reused as
                    // the sxtw/mul accumulator.
                    let other = if matches!(base, Register::Physical(r) if r == SCRATCH0) {
                        Register::Physical(SCRATCH1)
                    } else {
                        Register::Physical(SCRATCH0)
                    };
                    let tmp0_s = self.reg_name(base, RegisterSize::X64);
                    let tmp1_s = self.reg_name(other, RegisterSize::X64);
                    self.emit_sub_sp(SCRATCH_SPILL_SLOT)?;
                    writeln!(self.writer, "\tstr {tmp0_s}, [sp]")?;
                    writeln!(self.writer, "\tsxtw {tmp0_s}, {idx_s}")?;
                    self.emit_mov_imm(&tmp1_s, scale as u64)?;
                    writeln!(self.writer, "\tmul {tmp0_s}, {tmp0_s}, {tmp1_s}")?;
                    writeln!(self.writer, "\tldr {tmp1_s}, [sp]")?;
                    self.emit_add_sp(SCRATCH_SPILL_SLOT)?;
                    writeln!(self.writer, "\tadd {dst_s}, {tmp1_s}, {tmp0_s}")?;
                } else {
                    let scratch0 = self.reg_name(Register::Physical(SCRATCH0), RegisterSize::X64);
                    let scratch1 = self.reg_name(Register::Physical(SCRATCH1), RegisterSize::X64);
                    writeln!(self.writer, "\tsxtw {scratch0}, {idx_s}")?;
                    self.emit_mov_imm(&scratch1, scale as u64)?;
                    writeln!(self.writer, "\tmul {scratch0}, {scratch0}, {scratch1}")?;
                    writeln!(self.writer, "\tadd {dst_s}, {base_s}, {scratch0}")?;
                }
            }
        }
        Ok(())
    }

    fn emit_cmp(&mut self, size: RegisterSize, lhs: Register, rhs: InstOperand) -> Result<(), Error> {
        let lhs_s = self.reg_name(lhs, size);
        match rhs {
            InstOperand::Register(r) => {
                writeln!(self.writer, "\tcmp {lhs_s}, {}", self.reg_name(r, size))?
            }
            InstOperand::Immediate(imm) if self.is_addsub_imm_encodable(imm) => {
                writeln!(self.writer, "\tcmp {lhs_s}, #{imm}")?
            }
            InstOperand::Immediate(imm) => {
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
        let dst_s = self.reg_name(dst, RegisterSize::X64);
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

        // The first non-zero 16-bit chunk is emitted with movz (which
        // zeroes the rest of the register); later chunks merge with movk.
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
        let dst_s = self.reg_name(dst, RegisterSize::X64);
        let base_s = self.reg_name(base, RegisterSize::X64);

        match offset {
            0 => writeln!(self.writer, "\tmov {dst_s}, {base_s}")?,
            off if off > 0 && self.is_addsub_imm_encodable(off) => {
                writeln!(self.writer, "\tadd {dst_s}, {base_s}, #{off}")?
            }
            off if off < 0 && self.is_addsub_imm_encodable(-off) => {
                writeln!(self.writer, "\tsub {dst_s}, {base_s}, #{}", -off)?
            }
            off => {
                let scratch_s = self.reg_name(scratch, RegisterSize::X64);
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
            let scratch = self.reg_name(Register::Physical(SCRATCH0), RegisterSize::X64);
            self.emit_mov_imm(&scratch, imm as u64)?;
            writeln!(self.writer, "\tadd sp, sp, {scratch}")?;
        }
        Ok(())
    }

    /// Emits a single-precision floating-point binary operation.  The
    /// asmt-4 solution maps the four [`FBinOp`] variants directly to the
    /// aarch64 mnemonics `fadd` / `fsub` / `fmul` / `fdiv`.
    fn emit_fbinop(
        &mut self,
        _op: FBinOp,
        _dst: Register,
        _lhs: Register,
        _rhs: Register,
    ) -> Result<(), Error> {
        todo!("asmt-4: emit fadd/fsub/fmul/fdiv s_d, s_n, s_m")
    }

    /// Emits a single-precision floating-point comparison `fcmp s_n, s_m`.
    /// The result is implicit in NZCV and is consumed by a subsequent
    /// `BCond` arm.
    fn emit_fcmp(&mut self, _lhs: Register, _rhs: Register) -> Result<(), Error> {
        todo!("asmt-4: emit fcmp s_n, s_m")
    }

    /// Emits `scvtf s_d, w_n` — signed 32-bit integer to single-precision
    /// float conversion.
    fn emit_scvtf(&mut self, _dst: Register, _src: Register) -> Result<(), Error> {
        todo!("asmt-4: emit scvtf s_d, w_n")
    }

    /// Emits `fcvtzs w_d, s_n` — single-precision float to signed 32-bit
    /// integer conversion (truncating toward zero).
    fn emit_fcvtzs(&mut self, _dst: Register, _src: Register) -> Result<(), Error> {
        todo!("asmt-4: emit fcvtzs w_d, s_n")
    }

    /// Emits `fmov s_d, s_n` (Fpr-to-Fpr) or `fmov s_d, w_n`
    /// (Gpr-to-Fpr) depending on the source operand's register class.
    fn emit_fmov(&mut self, _dst: Register, _src: InstOperand) -> Result<(), Error> {
        todo!("asmt-4: emit fmov s_d, {{s|w}}_n")
    }

    /// Caller-saved bracket of both allocation pools.  Paired with
    /// [`Self::emit_restore_caller_regs`] around every `bl`; the
    /// registers are the same sets that `register_allocator` colours
    /// into, so any of them may be live across the call.  Each `stp` /
    /// `str` consumes 16 bytes of stack to keep `sp` 16-byte aligned,
    /// even when only 8 bytes are actually used.
    ///
    /// The integer half (`x8`–`x15`) is always emitted.  The FP half
    /// (`d18`–`d25`, the low-32 halves carrying the live `f32`) is
    /// emitted only when the current function uses FP, so a function
    /// that never touches FP emits no FP save / restore around its calls.
    ///
    /// This is one of the four sites in the backend that emit the
    /// AAPCS64 frame contract as raw assembly text; see
    /// [`Self::emit_prologue`] for the others.
    fn emit_save_caller_regs(&mut self) -> Result<(), Error> {
        writeln!(self.writer, "\tstr x15, [sp, #-16]!")?;
        writeln!(self.writer, "\tstp x13, x14, [sp, #-16]!")?;
        writeln!(self.writer, "\tstp x11, x12, [sp, #-16]!")?;
        writeln!(self.writer, "\tstp x9,  x10, [sp, #-16]!")?;
        writeln!(self.writer, "\tstr x8,  [sp, #-16]!")?;
        if self.current_fn_uses_fp {
            todo!(
                "asmt-4: preserve the caller-saved FP pool (d18-d25) here, \
                 pushed as `d` pairs to keep sp 16-byte aligned; see asmt-4.md §3.3"
            );
        }
        Ok(())
    }

    /// Mirror of [`Self::emit_save_caller_regs`]: pops both bands off
    /// the stack in reverse order so the caller resumes with the same
    /// register state it had before the `bl`.  The FP half is popped
    /// first (it was pushed last) and only when the function uses FP.
    fn emit_restore_caller_regs(&mut self) -> Result<(), Error> {
        if self.current_fn_uses_fp {
            todo!(
                "asmt-4: restore the caller-saved FP pool (d18-d25) here, \
                 in reverse of emit_save_caller_regs; see asmt-4.md §3.3"
            );
        }
        writeln!(self.writer, "\tldr x8,  [sp], #16")?;
        writeln!(self.writer, "\tldp x9,  x10, [sp], #16")?;
        writeln!(self.writer, "\tldp x11, x12, [sp], #16")?;
        writeln!(self.writer, "\tldp x13, x14, [sp], #16")?;
        writeln!(self.writer, "\tldr x15, [sp], #16")?;
        Ok(())
    }

    /// Inverse of [`Self::emit_prologue`]: restores `sp` to its
    /// post-prologue position (i.e. discards the local frame), pops the
    /// saved fp/lr pair, and branches to the link register.  Emitted by
    /// the [`Instruction::Ret`] arm of [`Self::emit_inst`].
    fn emit_epilogue(&mut self) -> Result<(), Error> {
        writeln!(self.writer, "\tmov sp, x29")?;
        writeln!(self.writer, "\tldp x29, x30, [sp], #16")?;
        writeln!(self.writer, "\tret")?;
        Ok(())
    }

    fn emit_inst(&mut self, inst: &Instruction) -> Result<(), Error> {
        match inst {
            Instruction::Label(name) => self.emit_label(name)?,
            Instruction::Mov { size, dst, src } => self.emit_mov(*size, *dst, *src)?,
            Instruction::BinOp {
                op,
                size,
                dst,
                lhs,
                rhs,
            } => self.emit_binop(*op, *size, *dst, *lhs, *rhs)?,
            Instruction::FBinOp { op, dst, lhs, rhs } => self.emit_fbinop(*op, *dst, *lhs, *rhs)?,
            Instruction::Ldr { size, dst, addr } => self.emit_load(*size, *dst, addr)?,
            Instruction::Str { size, src, addr } => self.emit_store(*size, *src, addr)?,
            Instruction::Lea { dst, addr } => self.emit_lea(*dst, addr)?,
            Instruction::Gep {
                dst,
                base,
                index,
                scale,
            } => self.emit_gep(*dst, *base, *index, *scale)?,
            Instruction::Cmp { size, lhs, rhs } => self.emit_cmp(*size, *lhs, *rhs)?,
            Instruction::FCmp { lhs, rhs } => self.emit_fcmp(*lhs, *rhs)?,
            Instruction::Scvtf { dst, src } => self.emit_scvtf(*dst, *src)?,
            Instruction::Fcvtzs { dst, src } => self.emit_fcvtzs(*dst, *src)?,
            Instruction::Fmov { dst, src } => self.emit_fmov(*dst, *src)?,
            Instruction::B { label } => writeln!(self.writer, "\tb {label}")?,
            Instruction::BCond { cond, label } => {
                writeln!(self.writer, "\tb.{} {label}", self.cond_suffix(*cond))?
            }
            Instruction::Bl { func } => writeln!(self.writer, "\tbl {func}")?,
            Instruction::SaveCallerRegs => self.emit_save_caller_regs()?,
            Instruction::RestoreCallerRegs => self.emit_restore_caller_regs()?,
            Instruction::SubSp { imm } => self.emit_sub_sp(*imm)?,
            Instruction::AddSp { imm } => self.emit_add_sp(*imm)?,
            Instruction::Ret => self.emit_epilogue()?,
        }
        Ok(())
    }

    fn emit_insts(&mut self, insts: &[Instruction]) -> Result<(), Error> {
        for inst in insts {
            self.emit_inst(inst)?;
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
            let scratch = self.reg_name(Register::Physical(SCRATCH0), RegisterSize::X64);
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

    /// Writes the AAPCS64 function prologue: pushes the saved fp/lr
    /// pair into the [`super::frame::SAVED_REGS_BYTES`] region atop
    /// the local frame, anchors `fp` at the new top of stack, and
    /// reserves `frame_size` bytes below it for the spill and alloca
    /// slots that [`super::frame::FrameLayout`] has laid out.
    ///
    /// Together with [`AsmPrinter::emit_epilogue`], the
    /// [`AsmPrinter::emit_save_caller_regs`] /
    /// [`AsmPrinter::emit_restore_caller_regs`] pair, and the
    /// `SubSp` / `AddSp` arms of [`Self::emit_inst`], this is the
    /// only set of sites that render the AAPCS64 frame contract as
    /// raw assembly text.  Every fp- or sp-relative offset they
    /// encode is fixed by [`super::frame`]'s constants.
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
