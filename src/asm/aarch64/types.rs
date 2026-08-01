//! Core type definitions for the AArch64 backend: the AAPCS64
//! register-role constants, registers, operands, addressing modes,
//! conditions, and the dtype-to-register-width mapping.
//!
//! The names are instruction-local (`InstOperand`, `InstBinOp`) so they
//! cannot be confused with the front-end's `ir::Operand` or
//! `ir::stmt::ArithBinOp`.

use crate::asm::error::Error;
use crate::ir;

// AAPCS64 register identifiers used throughout the aarch64 backend.
// The numeric values are the ARM architectural register indices and feed
// directly into `Register::Physical(_)`.  Bank (`x`/`w`/`s`) is implicit
// in the `RegisterSize` that accompanies the operand.

/// First integer argument register and integer return register (`x0`).
pub const REG_X0: u8 = 0;

/// Number of integer registers used for argument passing (`x0`–`x7`);
/// integer arguments beyond this go on the stack.  Tracks AAPCS64's
/// NGRN counter (§6.4.2 Stage C); independent of [`NUM_FP_ARG_REGS`].
pub const NUM_INT_ARG_REGS: u8 = 8;

/// First floating-point argument register and FP return register
/// (`s0` / `d0` / `v0`).  The FP register file is architecturally
/// independent of the GPR file: pairing `Register::Physical(REG_S0)`
/// with `RegisterSize::S32` denotes `s0`, while pairing the same physical
/// index with `RegisterSize::W32` / `RegisterSize::X64` denotes `w0` / `x0`
/// (i.e. [`REG_X0`]).
///
/// Consumed by the AAPCS64 FP argument / return shim (asmt-4.md §3.3).
#[allow(dead_code)]
pub const REG_S0: u8 = 0;

/// Number of FP registers used for argument passing (`s0`–`s7`);
/// floating-point arguments beyond this go on the stack.  Tracks
/// AAPCS64's NSRN counter (§6.4.2 Stage C); independent of
/// [`NUM_INT_ARG_REGS`].
pub const NUM_FP_ARG_REGS: u8 = 8;

/// First intra-procedure-call temporary (`x16`).  Reserved by AAPCS64 as
/// a scratch register for code generation; the aarch64 backend uses it
/// as `SCRATCH0`.
pub const REG_IP0: u8 = 16;

/// Second intra-procedure-call temporary (`x17`).  Used as `SCRATCH1`.
pub const REG_IP1: u8 = 17;

/// Frame pointer (`x29`).  Every prologue sets `x29 = sp`, and stack
/// addressing in the function body is fp-relative.
pub const REG_FP: u8 = 29;

/// Scratch register reserved for the code generator (`x16` / `w16` /
/// `s16`).  Alias of `REG_IP0`; the role-based name keeps spill/reload
/// sites readable.
pub const SCRATCH0: u8 = REG_IP0;

/// Second scratch register (`x17` / `w17` / `s17`).  Alias of `REG_IP1`.
pub const SCRATCH1: u8 = REG_IP1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Register {
    Virtual(usize),
    Physical(u8),
    StackPointer,
}

/// The hardware register class a virtual or physical register belongs to.
///
/// aarch64 keeps integer (`Gpr` — `w_`/`x_`) and floating-point/SIMD
/// (`Fpr` — `s_`/`d_`/`v_`) register banks completely independent: no
/// instruction can simultaneously source one operand from each bank.
/// The register allocator uses this class to split vregs into two
/// interference graphs that are coloured against disjoint pools.  The
/// skeleton's integer-only colouring constructs no variants, hence the
/// dead-code allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum RegisterClass {
    /// General-purpose integer (`x0`–`x30`, `sp`).
    Gpr,
    /// Floating-point / SIMD (`s0`–`s31`, alternatively `d`/`v`).
    Fpr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterSize {
    /// 32-bit general-purpose: `w0`–`w30`, also used for `i1` operands.
    W32,
    /// 64-bit general-purpose: `x0`–`x30`, also used for pointer-typed
    /// operands.
    X64,
    /// 32-bit single-precision floating-point: `s0`–`s31`.
    #[allow(dead_code)]
    S32,
}

impl RegisterSize {
    /// The register class implied by this width.  `W32`/`X64` live in
    /// the general-purpose bank; `S32` is a floating-point register.
    /// Consumed by the register allocator's class bucketing
    /// (asmt-4.md §3.4).
    #[allow(dead_code)]
    pub fn class(&self) -> RegisterClass {
        match self {
            RegisterSize::W32 | RegisterSize::X64 => RegisterClass::Gpr,
            RegisterSize::S32 => RegisterClass::Fpr,
        }
    }
}

/// Maps an IR scalar type onto the register width its operands occupy:
/// `i1`/`i32` live in a `w` register, pointers in an `x` register.
/// Aggregate and `void` types have no scalar register form and are
/// rejected.  The match is exhaustive so that introducing a new scalar
/// `Dtype` variant (e.g. `F32 -> S32`) fails to compile until handled.
impl TryFrom<&ir::Dtype> for RegisterSize {
    type Error = Error;

    fn try_from(dtype: &ir::Dtype) -> Result<Self, Self::Error> {
        match dtype {
            ir::Dtype::I1 | ir::Dtype::I32 => Ok(RegisterSize::W32),
            ir::Dtype::Pointer { .. } => Ok(RegisterSize::X64),
            ir::Dtype::Void | ir::Dtype::Struct { .. } | ir::Dtype::Array { .. } => {
                Err(Error::UnsupportedDtype {
                    dtype: dtype.clone(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstBinOp {
    Add,
    Sub,
    Mul,
    SDiv,
}

/// Single-precision floating-point binary operators corresponding 1:1
/// to the aarch64 `fadd`/`fsub`/`fmul`/`fdiv` instructions.  Unused by
/// the skeleton; asmt-4's solution produces them from the IR's
/// `FBiOpStmt`.  The `F` prefix mirrors the aarch64 mnemonic family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, clippy::enum_variant_names)]
pub enum FBinOp {
    FAdd,
    FSub,
    FMul,
    FDiv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// One operand of an integer instruction: a register or a 64-bit
/// immediate.  Also serves as a `Gep` index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstOperand {
    Register(Register),
    Immediate(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    BaseOff { base: Register, offset: i64 },
    Global(String),
}
