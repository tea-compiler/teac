//! AST (Abstract Syntax Tree) module for the TeaLang compiler.
//!
//! This module defines the complete structure of the AST produced by the parser.
//! It is organized into the following sub-modules:
//!
//! - [`decl`]: Declarations and definitions, including variable declarations,
//!   variable definitions, struct definitions, and function declarations/definitions.
//! - [`display`]: [`std::fmt::Display`] implementations for pretty-printing AST nodes.
//! - [`expr`]: Expression nodes, including arithmetic expressions, boolean expressions,
//!   comparison expressions, function calls, left-values, and right-values.
//! - [`ops`]: Operator enumerations for arithmetic, boolean, and comparison operations.
//! - [`program`]: Top-level program structure, including `use` statements and program elements.
//! - [`stmt`]: Statement nodes, including assignment, control flow (`if`, `while`),
//!   function calls, `return`, `break`, `continue`, and null statements.
//! - [`tree`]: AST traversal and visitor utilities.
//! - [`types`]: Type specifiers and built-in type definitions.
//!
//! All major types from sub-modules are re-exported at this level for convenient access.

pub mod decl;
pub mod display;
pub mod expr;
pub mod ops;
pub mod program;
pub mod stmt;
pub mod tree;
pub mod types;

pub use types::{BuiltIn, TypeSpecifier, TypeSpecifierInner};

pub use ops::{ArithBiOp, BoolBiOp, BoolUOp, ComOp};

pub use expr::{
    ArithBiOpExpr, ArithExpr, ArithExprInner, ArrayExpr, BoolBiOpExpr, BoolExpr, BoolExprInner,
    BoolUOpExpr, BoolUnit, BoolUnitInner, ComExpr, ExprUnit, ExprUnitInner, FnCall, IndexExpr,
    IndexExprInner, LeftVal, LeftValInner, MemberExpr, RightVal, RightValInner, RightValList,
};

pub use stmt::{
    AssignmentStmt, BreakStmt, CallStmt, CodeBlockStmt, CodeBlockStmtInner, ContinueStmt, IfStmt,
    NullStmt, ReturnStmt, WhileStmt,
};

pub use decl::{
    ArrayInitializer, FnDecl, FnDeclStmt, FnDef, ParamDecl, StructDef, VarDecl, VarDeclArray,
    VarDeclInner, VarDeclStmt, VarDeclStmtInner, VarDef, VarDefArray, VarDefInner, VarDefScalar,
};

pub use program::{Program, ProgramElement, ProgramElementInner, UseStmt};
