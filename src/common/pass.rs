//! Generic pass infrastructure shared by IR generation and optimization.
//!
//! teac uses two tiers of passes:
//!
//! - **Module passes** ([`ModulePass`]) run once over the whole program
//!   with `&mut IrGenerator<'_>`.  Used for cross-function analysis such
//!   as return-type inference.
//! - **Function passes** ([`FunctionPass`]) run on a single IR
//!   [`Function`].  Used for optimizations; see [`crate::opt`].
//!
//! Each tier has a matching [`ModulePassManager`] / [`FunctionPassManager`]
//! that runs a list of boxed trait objects in registration order.

use crate::ir::module::IrGenerator;
use crate::ir::{Error, Function};

// ---------------------------------------------------------------------------
// Module-level passes
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Function-level passes
// ---------------------------------------------------------------------------

/// A pass that runs on a single IR [`Function`].
///
/// Function passes are infallible: they either transform the function in
/// place or leave it unchanged.
pub trait FunctionPass {
    /// Apply this pass to `func` in place.
    fn run(&self, func: &mut Function);
}

/// Sequential pipeline of [`FunctionPass`] trait objects, executed in
/// registration order.
#[derive(Default)]
pub struct FunctionPassManager {
    passes: Vec<Box<dyn FunctionPass>>,
}

impl FunctionPassManager {
    /// Create an empty manager with no registered passes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `pass` to the end of the pipeline.
    pub fn add_pass(&mut self, pass: Box<dyn FunctionPass>) {
        self.passes.push(pass);
    }

    /// Run every pass against `func` in registration order.
    pub fn run(&self, func: &mut Function) {
        for pass in &self.passes {
            pass.run(func);
        }
    }
}
