//! Assembly code generation backend: translates the compiler's IR
//! into target-specific assembly.

pub mod aarch64;
pub mod common;
pub mod error;

pub use aarch64::AArch64AsmGenerator;
