//! AAPCS64 §6.4.2 argument-passing classification.
//!
//! [`classify_args`] walks one argument-dtype list and assigns each
//! entry to its canonical AAPCS64 location ([`ArgumentLocation`]).  Both the
//! callee's entry shim (lifts incoming values out of `x_`/`s_`/stack
//! into vregs) and the caller's call-site shim (settles outgoing
//! values into the same locations before `bl`) consume the same
//! classification result, so the two sides stay in lock-step by
//! construction.
//!
//! Two independent counters advance during the walk:
//!
//! * **NGRN** (Next General Register Number) — consumes
//!   `x0`–`x{NUM_INT_ARG_REGS - 1}` for integer-class arguments
//!   (`i1`, `i32`, pointers, array references).
//! * **NSRN** (Next SIMD/FP Register Number) — consumes
//!   `s0`–`s{NUM_FP_ARG_REGS - 1}` for floating-point arguments.
//!
//! When the corresponding counter saturates, further arguments of
//! that class spill onto the stack at successive
//! [`OUTGOING_ARG_SLOT_BYTES`]-wide slots in source order.  The slot
//! stride is fixed at 8 because every TeaLang argument type is at
//! most 8 bytes wide and AAPCS64 rounds each stack slot up to an
//! 8-byte boundary.
//!
//! Frame-layout sizing of the outgoing-arg area lives in
//! [`super::frame`]; this module only emits the per-argument
//! classification that the layout consumes.

use super::types::{NUM_FP_ARG_REGS, NUM_INT_ARG_REGS};
use crate::asm::error::Error;
use crate::ir;

/// Width of one stack-passed argument slot.  AAPCS64 §6.4.2 rounds
/// every stack slot up to 8 bytes; TeaLang has no argument type wider
/// than 8 bytes, so the slot is always exactly 8.  The classifier
/// advances its stack cursor by this stride, and the frame layer
/// sizes the outgoing-arg area in the same unit.
pub const OUTGOING_ARG_SLOT_BYTES: i64 = 8;

/// AAPCS64 location of a single argument at the call boundary.
///
/// `Gpr(n)` / `Fpr(n)` carry the architectural register index; pair
/// the index with the operand's [`super::types::RegisterSize`] to
/// obtain the concrete `w_`/`x_`/`s_` form.
///
/// `Stack { offset }` is the byte offset within the outgoing-arg
/// area.  The two sides interpret the offset against different bases:
///
/// | Side    | Effective address                                            |
/// | ------- | ------------------------------------------------------------ |
/// | caller  | `[sp, #offset]`                                              |
/// | callee  | `[fp, #offset + super::frame::STACK_ARG_FP_BASE]`            |
///
/// where `STACK_ARG_FP_BASE` accounts for the saved fp/lr pair that
/// every prologue places between fp and the incoming-arg area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentLocation {
    Gpr(u8),
    Fpr(u8),
    Stack { offset: i64 },
}

/// Walks `dtypes` once and returns the AAPCS64 location of each
/// argument, advancing NGRN / NSRN as documented above.
///
/// Returns [`Error::UnsupportedDtype`] for any dtype that cannot
/// legally appear at a call boundary (`void`, struct-by-value,
/// ...).
pub fn classify_args<'a, I>(dtypes: I) -> Result<Vec<ArgumentLocation>, Error>
where
    I: IntoIterator<Item = &'a ir::Dtype>,
{
    let mut ngrn: u8 = 0;
    let mut nsrn: u8 = 0;
    let mut stack_off: i64 = 0;

    dtypes
        .into_iter()
        .map(|dtype| {
            let loc = match ArgumentClass::try_from(dtype)? {
                ArgumentClass::Int => take_or_spill(&mut ngrn, NUM_INT_ARG_REGS, &mut stack_off, ArgumentLocation::Gpr),
                ArgumentClass::Float => take_or_spill(&mut nsrn, NUM_FP_ARG_REGS, &mut stack_off, ArgumentLocation::Fpr),
            };
            Ok(loc)
        })
        .collect()
}

/// AAPCS64 argument class — selects which of NGRN / NSRN the argument
/// advances.  Kept internal to this module; callers consume the
/// resolved [`ArgumentLocation`] instead.
///
/// `Float` is unreachable while [`ir::Dtype`] lacks an `F32` variant;
/// the arm is kept as part of the classifier's AAPCS64 specification
/// so adding the variant only extends the `TryFrom<&ir::Dtype>` impl.
enum ArgumentClass {
    Int,
    #[allow(dead_code)]
    Float,
}

impl TryFrom<&ir::Dtype> for ArgumentClass {
    type Error = Error;

    fn try_from(dtype: &ir::Dtype) -> Result<Self, Self::Error> {
        match dtype {
            ir::Dtype::I1
            | ir::Dtype::I32
            | ir::Dtype::Pointer { .. }
            | ir::Dtype::Array { .. } => Ok(Self::Int),
            ir::Dtype::Void | ir::Dtype::Struct { .. } => Err(Error::UnsupportedDtype {
                dtype: dtype.clone(),
            }),
        }
    }
}

/// Allocates the next slot of one register class — returning a
/// `Reg(n)` `ArgumentLocation` until the class saturates, then a
/// `Stack { offset }` `ArgumentLocation` constructed from the running stack
/// cursor.  Both cursors are mutated in place.
fn take_or_spill(
    cursor: &mut u8,
    cap: u8,
    stack_off: &mut i64,
    reg_ctor: fn(u8) -> ArgumentLocation,
) -> ArgumentLocation {
    if *cursor < cap {
        let n = *cursor;
        *cursor += 1;
        reg_ctor(n)
    } else {
        let off = *stack_off;
        *stack_off += OUTGOING_ARG_SLOT_BYTES;
        ArgumentLocation::Stack { offset: off }
    }
}
