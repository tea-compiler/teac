//! Top-level program AST nodes.
//!
//! This module defines the root `Program` node and the elements that can
//! appear at the top level of a source file: `use` import statements,
//! variable declarations/definitions, struct definitions, function forward
//! declarations, and function definitions.

use super::decl::{FnDeclStmt, FnDef, StructDef, VarDeclStmt};

/// A `use` statement that imports an external module into the current scope,
/// e.g. `use io;`.
#[derive(Debug, Clone)]
pub struct UseStmt {
    /// The name of the module being imported.
    pub module_name: String,
}

/// The inner kind of a top-level program element.
#[derive(Debug, Clone)]
pub enum ProgramElementInner {
    /// A global variable declaration or definition.
    VarDeclStmt(Box<VarDeclStmt>),
    /// A struct type definition.
    StructDef(Box<StructDef>),
    /// A function forward declaration (prototype).
    FnDeclStmt(Box<FnDeclStmt>),
    /// A function definition with a body.
    FnDef(Box<FnDef>),
}

/// A single top-level element in a program, wrapping its specific kind.
#[derive(Debug, Clone)]
pub struct ProgramElement {
    /// The actual top-level element content.
    pub inner: ProgramElementInner,
}

/// An ordered list of top-level program elements.
pub type ProgramElementList = Vec<ProgramElement>;

/// The root node of the AST, representing a complete source file.
#[derive(Debug, Clone)]
pub struct Program {
    /// The `use` import statements at the top of the file.
    pub use_stmts: Vec<UseStmt>,
    /// The top-level declarations, definitions, and function bodies.
    pub elements: ProgramElementList,
}
