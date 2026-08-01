//! This module defines the compiler's intermediate representation (IR).
//!
//! The IR is the central data structure that bridges the front-end (parsing
//! and type-checking) and the back-end (optimization and code generation).

pub mod error;
pub mod function;
mod gen;
pub mod module;
pub mod pass;
pub mod printer;
pub mod stmt;
pub mod types;
pub mod value;

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::ast;

/// Resolves a source-level function name to the symbol name exposed to
/// the linker.
///
/// teac uses namespace-qualified function names at the source level
/// (`std::putint`, `Counter::get`) to keep the human-facing name space
/// clean, but the linker only sees flat C-style symbols.  This function
/// is the single place where that mapping is defined; both the LLVM IR
/// printer and the AArch64 assembler consume its output verbatim.
///
/// Three rules, chosen to be orthogonal and stable:
///
/// * **External functions** (`is_external = true`, i.e. declarations
///   coming from a `.teah` header) are mapped to their last path
///   segment.  This mirrors C++ `extern "C"` blocks: the source-level
///   name is namespaced for the programmer, while the link-level name
///   matches the runtime's bare C symbol (`std::putint` -> `putint`).
///
/// * **Unqualified user functions** (`a_single_ident`) are exported
///   unchanged.  This preserves the natural identity for the
///   overwhelmingly common case.
///
/// * **Qualified user functions** (e.g. `Counter::get` produced by an
///   `impl` block) are encoded using an Itanium-C++-ABI-inspired
///   scheme: `_TN` + length-prefixed segments + `E`, so
///   `Counter::get` -> `_TN7Counter3getE`.  The `_T` vendor prefix
///   avoids collisions with real C++ symbols (`_Z...`) while keeping
///   the mangling unambiguous and reversible.
pub(crate) fn compute_link_name(source_name: &str, is_external: bool) -> String {
    if is_external {
        source_name
            .rsplit("::")
            .next()
            .expect("rsplit on a non-empty str always yields at least one segment")
            .to_string()
    } else if source_name.contains("::") {
        let mut mangled = String::from("_TN");
        for segment in source_name.split("::") {
            write!(mangled, "{}{segment}", segment.len())
                .expect("writing into a String cannot fail");
        }
        mangled.push('E');
        mangled
    } else {
        source_name.to_string()
    }
}

pub use error::Error;
pub use function::{BasicBlock, BlockLabel, Function, FunctionBody};
pub use module::{IrGenerator, Module, Registry};
// `#[allow(unused_imports)]` because the only in-tree consumer of this
// re-export (the `experimental::ReturnInferPass` implementation) is gated
// on the `return-type-inference` feature.
#[allow(unused_imports)]
pub use pass::ModulePass;
pub use types::{Dtype, StructType};
pub use value::{GlobalDef, Local, LocalId, Operand};

#[cfg(feature = "return-type-inference")]
pub(crate) use crate::experimental::ReturnInferPass;

/// Crate-internal helper surfaced for the `experimental` layer, which
/// lives outside `mod gen` and therefore cannot reach into private
/// submodules directly.  Not part of the public `ir` API.
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
