//! Post-IR optimization: the [`Optimizer`] and its function-level passes.

pub mod cfg;
mod dominator;
mod mem2reg;

pub use mem2reg::Mem2RegPass;

pub use crate::common::pass::FunctionPass;

use std::io::Write;

use crate::common::pass::FunctionPassManager;
use crate::common::Generator;
use crate::ir::printer::IrPrinter;
use crate::ir::{Error, Module, Registry};

/// Driver for teac's post-IR optimization pipeline.
///
/// Holds an ordered pipeline of [`FunctionPass`] trait objects and
/// applies them to every function body on [`Generator::generate`].
pub struct Optimizer<'a> {
    passes: FunctionPassManager,
    module: &'a mut Module,
    registry: &'a Registry,
}

impl<'a> Optimizer<'a> {
    /// Create an optimizer with no passes registered.
    pub fn new(module: &'a mut Module, registry: &'a Registry) -> Self {
        Self {
            passes: FunctionPassManager::new(),
            module,
            registry,
        }
    }

    /// Create an optimizer pre-loaded with [`install_default_passes`].
    pub fn with_default_passes(module: &'a mut Module, registry: &'a Registry) -> Self {
        let mut o = Self::new(module, registry);
        install_default_passes(&mut o);
        o
    }

    /// Append `pass` to the end of the pipeline.
    pub fn add_function_pass(&mut self, pass: Box<dyn FunctionPass>) {
        self.passes.add_pass(pass);
    }
}

impl Generator for Optimizer<'_> {
    type Error = Error;

    /// Run every registered pass against every function in the module.
    fn generate(&mut self) -> Result<(), Self::Error> {
        for func in self.module.function_list.values_mut() {
            self.passes.run(func);
        }
        Ok(())
    }

    /// Emit the (now-optimised) IR.
    fn output<W: Write>(&self, w: &mut W) -> Result<(), Self::Error> {
        IrPrinter::new(w).emit_module(self.module, self.registry)
    }
}

/// Install teac's default function-level pass pipeline on `optimizer`.
///
/// This is the single place teac's default optimization pass list is
/// written down.
pub fn install_default_passes(optimizer: &mut Optimizer<'_>) {
    optimizer.add_function_pass(Box::new(Mem2RegPass));
}
