//! Core IR data structures: the [`Module`], the [`Registry`] of type
//! definitions, and the [`IrGenerator`] that populates them.

use super::function::Function;
use super::types::FunctionType;
use super::value::GlobalDef;
use crate::ast;
use crate::common::pass::{ModulePass, ModulePassManager};
use indexmap::IndexMap;
use std::path::PathBuf;
use std::rc::Rc;

use super::types::StructType;

/// Type definitions shared across IR generation.
pub struct Registry {
    pub struct_types: IndexMap<String, StructType>,
    pub function_types: IndexMap<String, FunctionType>,
}

/// A compiled module: the top-level container for globals and functions.
pub struct Module {
    pub global_list: IndexMap<Rc<str>, GlobalDef>,
    pub function_list: IndexMap<String, Function>,
}

/// Transforms an AST program into IR.
pub struct IrGenerator<'a> {
    pub input: &'a ast::Program,
    /// Directory of the source file; used to resolve `use` imports.
    pub source_dir: PathBuf,
    pub module: Module,
    pub registry: Registry,
    /// Module-level plug-in pipeline, run between signature registration
    /// (Pass 2) and per-function IR generation (Pass 3).
    pub(crate) module_passes: ModulePassManager,
}

impl<'a> IrGenerator<'a> {
    /// Create a generator with empty module, registry, and pass pipeline.
    pub fn new(input: &'a ast::Program, source_dir: PathBuf) -> Self {
        let module = Module {
            global_list: IndexMap::new(),
            function_list: IndexMap::new(),
        };
        let registry = Registry {
            struct_types: IndexMap::new(),
            function_types: IndexMap::new(),
        };
        Self {
            input,
            source_dir,
            module,
            registry,
            module_passes: ModulePassManager::new(),
        }
    }

    /// Append a module-level pass to the pipeline.
    //
    // `#[allow(dead_code)]` because the only in-tree caller is gated on
    // the `return-type-inference` feature.
    #[allow(dead_code)]
    pub fn add_module_pass(&mut self, pass: Box<dyn ModulePass>) {
        self.module_passes.add_pass(pass);
    }
}
