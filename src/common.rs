//! Common utilities and shared abstractions used across the compiler,
//! including target platform detection and a generic code generator trait.

pub mod graph;

use std::io::Write;

/// Represents the compilation target operating system.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Compile for Linux.
    Linux,
    /// Compile for macOS.
    Macos,
}

impl Target {
    /// Detects the current host platform at compile time and returns the
    /// corresponding `Target` variant.
    pub fn host() -> Self {
        if cfg!(target_os = "macos") {
            Target::Macos
        } else {
            Target::Linux
        }
    }

    /// Applies platform-specific symbol name mangling.
    ///
    /// On macOS, the Mach-O ABI requires a leading underscore prefix for C
    /// symbols, so this method prepends `_` to the given name.  On Linux the
    /// name is returned unchanged.
    pub fn mangle_symbol(&self, name: &str) -> String {
        match self {
            Target::Macos => format!("_{name}"),
            Target::Linux => name.to_string(),
        }
    }
}

/// A generic trait for code generators.
///
/// Implementors first call [`generate`](Generator::generate) to perform the
/// code-generation work and then call [`output`](Generator::output) to write
/// the generated result to any [`Write`] sink.
pub trait Generator {
    type Error;
    /// Performs the code-generation step, populating the generator's internal
    /// state with the result.
    fn generate(&mut self) -> Result<(), Self::Error>;
    /// Writes the previously generated output to `w`.
    fn output<W: Write>(&self, w: &mut W) -> Result<(), Self::Error>;
}
