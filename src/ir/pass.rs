//! Module-level pass infrastructure: [`ModulePass`] and [`ModulePassManager`].
//!
//! teac uses two tiers of passes, split across layers so that the
//! layer-neutral [`crate::common`] utilities never depend upward on the IR:
//!
//! - **Module passes** ([`ModulePass`], this module) run once over the whole
//!   program with `&mut IrGenerator<'_>`.  Used for cross-function analysis
//!   such as return-type inference.
//! - **Function passes** ([`crate::common::pass::FunctionPass`]) run on a
//!   single IR [`crate::ir::Function`].  Used for optimizations; see
//!   [`crate::opt`].
//!
//! # Fallibility policy per tier
//!
//! Module passes are **fallible**: [`ModulePass::run`] returns
//! `Result<(), Error>` and the first error aborts the pipeline (subsequent
//! passes and later compilation stages are skipped).  Function passes are
//! **infallible by contract**: their `run` returns `()` and a panic means a
//! compiler bug.
//!
//! Each tier has a matching pass manager that runs a list of boxed trait
//! objects in registration order: [`ModulePassManager`] here and
//! [`crate::common::pass::FunctionPassManager`] for the other tier.

use super::error::Error;
use super::module::IrGenerator;

/// A pass that runs once over the whole translation unit.
pub trait ModulePass {
    /// Run this pass against `gen`.  Returning `Err` aborts the pipeline
    /// (subsequent passes and later compilation stages are skipped).
    fn run(&self, gen: &mut IrGenerator<'_>) -> Result<(), Error>;
}

/// Sequential pipeline of [`ModulePass`] trait objects, executed in
/// registration order.
#[derive(Default)]
pub struct ModulePassManager {
    passes: Vec<Box<dyn ModulePass>>,
}

impl ModulePassManager {
    /// Create an empty manager with no registered passes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `pass` to the end of the pipeline.
    pub fn add_pass(&mut self, pass: Box<dyn ModulePass>) {
        self.passes.push(pass);
    }

    /// Run every pass against `gen`, stopping at the first error.
    pub fn run(&self, gen: &mut IrGenerator<'_>) -> Result<(), Error> {
        for pass in &self.passes {
            pass.run(gen)?;
        }
        Ok(())
    }
}
