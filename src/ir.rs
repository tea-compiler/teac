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

use std::path::PathBuf;

use crate::ast;

pub use error::Error;
pub use function::{BasicBlock, BlockLabel, Function, FunctionBody};
pub use module::{IrGenerator, Module, Registry};
pub use types::{Dtype, StructType};
pub use value::{GlobalDef, Local, LocalId, Operand};

#[cfg(feature = "return-type-inference")]
pub(crate) use crate::experimental::ReturnInferPass;

// Crate-internal helper surfaced for the `experimental` layer, which
// lives outside `mod gen` and therefore cannot reach into private
// submodules directly.  Not part of the public `ir` API.
#[cfg(feature = "return-type-inference")]
pub(crate) use gen::conversions::compose_var_def_dtype;

/// Install teac's default module-level pass pipeline on `gen`.
///
/// This helper is the single home of the "which module passes does
/// teac run by default?" decision.  The body of the function is the
/// only place in the tree where `#[cfg(feature = ...)]` for module
/// passes needs to appear.  In other words, feature gates stay
/// co-located with the features they gate, and the driver code stays
/// agnostic.
///
/// Most callers want the convenience constructor
/// [`IrGenerator::with_default_passes`], which wraps
/// `IrGenerator::new` and `install_default_passes` into one call.
/// This standalone function stays exposed for the "compose defaults
/// with my own passes" case (useful in tests and ad-hoc debugging):
/// start from `IrGenerator::new`, register custom passes, then call
/// `install_default_passes` to bolt the defaults on top.
#[allow(unused_variables)]
pub fn install_default_passes(gen: &mut IrGenerator<'_>) {
    #[cfg(feature = "return-type-inference")]
    gen.add_module_pass(Box::new(ReturnInferPass));
}

impl<'a> IrGenerator<'a> {
    /// Convenience constructor: equivalent to [`IrGenerator::new`]
    /// followed by [`install_default_passes`].
    ///
    /// This is the entry point the driver (`main.rs`) uses to obtain
    /// an `IrGenerator` already wired up with teac's canonical
    /// module-pass pipeline.  Callers that want a *neutral* generator
    /// (no pre-registered passes — typical for unit tests that isolate
    /// a single pass, or for ad-hoc debugging) should keep using
    /// [`IrGenerator::new`] and call
    /// [`add_module_pass`](IrGenerator::add_module_pass) themselves.
    pub fn with_default_passes(input: &'a ast::Program, source_dir: PathBuf) -> Self {
        let mut g = Self::new(input, source_dir);
        install_default_passes(&mut g);
        g
    }
}
