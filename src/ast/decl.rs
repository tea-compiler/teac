//! Declaration and definition AST nodes.
//!
//! This module covers everything related to declaring or defining names in
//! the language: variable declarations and definitions (scalar and array),
//! struct definitions, function declarations, and function definitions.

use super::expr::{RightVal, RightValList};
use super::stmt::CodeBlockStmtList;
use super::types::TypeSpecifier;
use std::ops::Deref;

/// The fixed-length metadata for an array variable declaration.
#[derive(Debug, Clone)]
pub struct VarDeclArray {
    /// The number of elements in the array.
    pub len: usize,
}

/// Whether a variable declaration is a scalar or a fixed-length array.
#[derive(Debug, Clone)]
pub enum VarDeclInner {
    /// A scalar (non-array) variable declaration.
    Scalar,
    /// A fixed-length array variable declaration.
    Array(Box<VarDeclArray>),
}

/// A variable declaration — a name and optional type specifier without an
/// initial value.  Used in function parameter lists and as forward
/// declarations.
#[derive(Debug, Clone)]
pub struct VarDecl {
    /// The variable name.
    pub identifier: String,
    /// Optional explicit type annotation; `None` means the type is inferred.
    pub type_specifier: Option<TypeSpecifier>,
    /// Whether the declaration is for a scalar or an array.
    pub inner: VarDeclInner,
}

/// A list of variable declarations, used for struct fields and parameter lists.
pub type VarDeclList = Vec<VarDecl>;

/// The initializer for a scalar variable definition, holding its initial value.
#[derive(Debug, Clone)]
pub struct VarDefScalar {
    /// The initial value expression.
    pub val: Box<RightVal>,
}

/// The initializer for an array variable definition.
#[derive(Debug, Clone)]
pub enum ArrayInitializer {
    /// An explicit element-by-element initializer list, e.g. `[1, 2, 3]`.
    ExplicitList(RightValList),
    /// A fill initializer that repeats a single value `count` times,
    /// e.g. `[0; 10]`.
    Fill { val: Box<RightVal>, count: usize },
}

/// The initializer for a fixed-length array variable definition.
#[derive(Debug, Clone)]
pub struct VarDefArray {
    /// The declared length of the array.
    pub len: usize,
    /// The initializer (explicit list or fill).
    pub initializer: ArrayInitializer,
}

/// Whether a variable definition is for a scalar or an array.
#[derive(Debug, Clone)]
pub enum VarDefInner {
    /// A scalar variable definition with a single initial value.
    Scalar(Box<VarDefScalar>),
    /// An array variable definition with a length and initializer.
    Array(Box<VarDefArray>),
}

/// A variable definition — a name, optional type specifier, and an
/// initial value (scalar or array).
#[derive(Debug, Clone)]
pub struct VarDef {
    /// The variable name.
    pub identifier: String,
    /// Optional explicit type annotation.
    pub type_specifier: Option<TypeSpecifier>,
    /// The initial value (scalar or array).
    pub inner: VarDefInner,
}

/// A statement that either declares or defines a variable.
#[derive(Debug, Clone)]
pub enum VarDeclStmtInner {
    /// A declaration without an initial value.
    Decl(Box<VarDecl>),
    /// A definition with an initial value.
    Def(Box<VarDef>),
}

/// A top-level or block-scoped variable declaration/definition statement.
#[derive(Debug, Clone)]
pub struct VarDeclStmt {
    /// Whether this statement is a bare declaration or a definition.
    pub inner: VarDeclStmtInner,
}

/// A struct type definition, grouping a set of named fields.
#[derive(Debug, Clone)]
pub struct StructDef {
    /// The struct type name.
    pub identifier: String,
    /// The list of field declarations.
    pub decls: VarDeclList,
}

/// The formal parameter declaration of a function, consisting of one or more
/// named (and optionally typed) variable declarations.
#[derive(Debug, Clone)]
pub struct ParamDecl {
    /// The list of parameter variable declarations.
    pub decls: VarDeclList,
}

/// A function declaration (prototype) — name, optional parameters, and
/// optional return type, without a body.
#[derive(Debug, Clone)]
pub struct FnDecl {
    /// The function name.
    pub identifier: String,
    /// Optional parameter declaration; `None` means no parameters.
    pub param_decl: Option<Box<ParamDecl>>,
    /// Optional return type; `None` means the function returns nothing (void).
    pub return_dtype: Option<TypeSpecifier>,
}

/// A function definition — a declaration together with a body.
#[derive(Debug, Clone)]
pub struct FnDef {
    /// The function's declaration (name, parameters, return type).
    pub fn_decl: Box<FnDecl>,
    /// The ordered list of statements forming the function body.
    pub stmts: CodeBlockStmtList,
}

/// A function declaration used as a top-level statement (forward declaration).
#[derive(Debug, Clone)]
pub struct FnDeclStmt {
    /// The underlying function declaration.
    pub fn_decl: Box<FnDecl>,
}

/// `Deref` implementation so that `FnDeclStmt` can be used directly wherever
/// a `FnDecl` reference is expected, avoiding repeated `.fn_decl` field
/// accesses.
impl Deref for FnDeclStmt {
    type Target = FnDecl;

    /// Returns a reference to the inner `FnDecl`.
    fn deref(&self) -> &Self::Target {
        &self.fn_decl
    }
}
