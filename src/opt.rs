//! This module provides optimization passes that transform IR functions.
//!
//! Passes implement the [`FunctionPass`] trait and are composed into a
//! [`FunctionPassManager`] pipeline that runs them in registration order.

use crate::ir::function::Function;

pub mod cfg;
mod dominator;
mod mem2reg;

pub use mem2reg::Mem2RegPass;

/// Interface that every function-level optimization pass must implement.
pub trait FunctionPass {
    fn run(&self, func: &mut Function);
}

/// Manages a sequential pipeline of [`FunctionPass`] instances.
///
/// Passes are stored as boxed trait objects so that heterogeneous pass types
/// can be combined in a single pipeline.
#[derive(Default)]
pub struct FunctionPassManager {
    passes: Vec<Box<dyn FunctionPass>>,
}

impl FunctionPassManager {
    /// Creates an empty pass manager with no registered passes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a pass manager pre-loaded with the default optimization
    /// pipeline (currently: [`Mem2RegPass`]).
    pub fn with_default_pipeline() -> Self {
        let mut pm = Self::new();
        pm.add_pass(Mem2RegPass);
        pm
    }

    /// Appends `pass` to the end of the optimization pipeline.
    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: FunctionPass + 'static,
    {
        self.passes.push(Box::new(pass));
    }

    /// Runs all registered passes sequentially on `func`.
    pub fn run(&self, func: &mut Function) {
        for pass in &self.passes {
            pass.run(func);
        }
    }
}
