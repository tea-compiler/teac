//! This module defines the compiler's intermediate representation (IR).
//!
//! The IR is the central data structure that bridges the front-end (parsing
//! and type-checking) and the back-end (optimization and code generation).

pub mod error;
pub mod function;
mod gen;
pub mod module;
pub mod printer;
pub mod stmt;
pub mod types;
pub mod value;

pub use error::Error;
pub use function::{BasicBlock, BlockLabel, Function};
pub use module::{IrGenerator, Module, Registry};
pub use types::{Dtype, StructType};
pub use value::{GlobalVariable, LocalRef, Operand};
