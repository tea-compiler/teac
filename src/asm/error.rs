//! Error type for the assembly backend: unsupported IR constructs,
//! missing layout or condition information, and internal invariant
//! violations raised during lowering or register allocation.

use thiserror::Error;

use crate::ir;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error")]
    Io(#[from] std::io::Error),

    #[error("unsupported data type: {dtype:?}")]
    UnsupportedDtype { dtype: ir::Dtype },

    #[error("unsupported IR operand: {what}")]
    UnsupportedOperand { what: String },

    #[error("missing condition mapping for virtual register %{vreg}")]
    MissingCond { vreg: usize },

    #[error("missing struct layout for '{name}'")]
    MissingStructLayout { name: String },

    #[error("invalid struct field index {index} for struct '{name}'")]
    InvalidStructFieldIndex { name: String, index: i64 },

    #[error("internal error: {0}")]
    Internal(String),
}
