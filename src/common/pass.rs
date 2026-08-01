//! Function-level pass infrastructure shared by IR generation and
//! optimization.
//!
//! teac uses two tiers of passes; this module holds the layer-neutral tier:
//!
//! - **Function passes** ([`FunctionPass`]) run on a single IR
//!   [`Function`].  Used for optimizations; see [`crate::opt`].
//! - **Module passes** ([`crate::ir::ModulePass`]) run once over the whole
//!   program with `&mut IrGenerator<'_>`.  They are IR-coupled by design and
//!   live in [`crate::ir::pass`].
//!
//! # Fallibility policy per tier
//!
//! Function passes are **infallible by contract**: [`FunctionPass::run`]
//! returns `()` and a panic means a compiler bug, not a recoverable error.
//! Module passes are **fallible**: they return `Result<(), ir::Error>` and
//! the first error aborts the pipeline.
//!
//! Each tier has a matching pass manager — [`FunctionPassManager`] here and
//! [`crate::ir::pass::ModulePassManager`] in the IR layer — that runs a list
//! of boxed trait objects in registration order.
//!
//! This module remains IR-coupled through [`FunctionPass`]/[`Function`];
//! the coupling is accepted because `common` is a crate-internal utility
//! layer, not a public API surface.

use crate::ir::Function;

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
