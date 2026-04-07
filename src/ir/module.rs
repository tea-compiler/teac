//! This module defines the core IR (Intermediate Representation) data structures,
//! including the Module, Registry, and IrGenerator used for code generation.

use super::function::Function;
use super::types::FunctionType;
use super::value::GlobalVariable;
use crate::ast;
use indexmap::IndexMap;
use std::path::PathBuf;

use super::types::StructType;

/// A registry that holds type definitions used during IR generation.
/// It stores struct type definitions and function type signatures
/// that are referenced throughout the compilation process.
pub struct Registry {
    /// A map of struct type names to their corresponding struct type definitions.
    pub struct_types: IndexMap<String, StructType>,
    /// A map of function type names to their corresponding function type signatures.
    pub function_types: IndexMap<String, FunctionType>,
}

/// Represents a compiled module containing all global variables and functions.
/// This is the top-level container for the generated IR output.
pub struct Module {
    /// A map of global variable names to their definitions.
    pub global_list: IndexMap<String, GlobalVariable>,
    /// A map of function names to their compiled function representations.
    pub function_list: IndexMap<String, Function>,
}

/// The main IR generator that transforms an AST program into IR.
/// It holds a reference to the input AST, the output module, and
/// a registry of type definitions used during the generation process.
pub struct IrGenerator<'a> {
    /// A reference to the input AST program to be compiled.
    pub input: &'a ast::Program,
    /// The directory containing the source file being compiled.
    /// Used to resolve module header files (e.g. `std.teah`) relative
    /// to the source file when processing `use` statements.
    pub source_dir: PathBuf,
    /// The output module that accumulates generated IR constructs.
    pub module: Module,
    /// The registry of type definitions available during IR generation.
    pub registry: Registry,
}

impl<'a> IrGenerator<'a> {
    /// The target triple specifying the architecture, vendor, and OS for code generation.
    pub(crate) const TARGET_TRIPLE: &'static str = "aarch64-unknown-linux-gnu";
    /// The target data layout string describing the memory layout conventions
    /// (endianness, alignment, pointer sizes, etc.) for the target platform.
    pub(crate) const TARGET_DATALAYOUT: &'static str =
        "e-m:e-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128";

    /// Creates a new `IrGenerator` with the given AST program as input
    /// and the directory containing the source file.
    /// The `source_dir` is used to resolve module header files when
    /// processing `use` statements.
    /// Initializes an empty module and an empty type registry.
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
        }
    }
}