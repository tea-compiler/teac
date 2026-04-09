//! Statement AST nodes.
//!
//! This module defines all statement kinds that can appear inside a function
//! body (code block): assignments, function-call statements, control-flow
//! statements (`if`, `while`, `return`, `continue`, `break`), variable
//! declarations, and the empty (null) statement.

use super::decl::VarDeclStmt;
use super::expr::{BoolUnit, FnCall, LeftVal, RightVal};

/// An assignment statement, e.g. `x = expr;`.
#[derive(Debug, Clone)]
pub struct AssignmentStmt {
    /// The target lvalue (the location being written to).
    pub left_val: Box<LeftVal>,
    /// The source rvalue (the value being assigned).
    pub right_val: Box<RightVal>,
}

/// A statement consisting of a bare function call whose return value is
/// discarded, e.g. `print(x);`.
#[derive(Debug, Clone)]
pub struct CallStmt {
    /// The function call expression.
    pub fn_call: Box<FnCall>,
}

/// A `return` statement, optionally carrying a value.
#[derive(Debug, Clone)]
pub struct ReturnStmt {
    /// The value to return, or `None` for a void return.
    pub val: Option<Box<RightVal>>,
}

/// A `continue` statement that jumps to the next iteration of the enclosing
/// loop.
#[derive(Debug, Clone)]
pub struct ContinueStmt {}

/// A `break` statement that exits the enclosing loop.
#[derive(Debug, Clone)]
pub struct BreakStmt {}

/// An empty (null) statement — a lone semicolon with no effect.
#[derive(Debug, Clone)]
pub struct NullStmt {}

/// An `if` statement, with a mandatory then-branch and an optional
/// else-branch.
#[derive(Debug, Clone)]
pub struct IfStmt {
    /// The condition that controls which branch is taken.
    pub bool_unit: Box<BoolUnit>,
    /// The statements executed when the condition is `true`.
    pub if_stmts: CodeBlockStmtList,
    /// The statements executed when the condition is `false`; absent if there
    /// is no `else` clause.
    pub else_stmts: Option<CodeBlockStmtList>,
}

/// A `while` loop statement.
#[derive(Debug, Clone)]
pub struct WhileStmt {
    /// The loop condition evaluated before each iteration.
    pub bool_unit: Box<BoolUnit>,
    /// The statements that form the loop body.
    pub stmts: CodeBlockStmtList,
}

/// The inner kind of a statement that can appear inside a code block.
#[derive(Debug, Clone)]
pub enum CodeBlockStmtInner {
    /// A variable declaration or definition.
    VarDecl(Box<VarDeclStmt>),
    /// An assignment statement.
    Assignment(Box<AssignmentStmt>),
    /// A function-call statement.
    Call(Box<CallStmt>),
    /// An `if` (possibly with `else`) statement.
    If(Box<IfStmt>),
    /// A `while` loop statement.
    While(Box<WhileStmt>),
    /// A `return` statement.
    Return(Box<ReturnStmt>),
    /// A `continue` statement.
    Continue(Box<ContinueStmt>),
    /// A `break` statement.
    Break(Box<BreakStmt>),
    /// An empty (null) statement.
    Null(Box<NullStmt>),
}

/// A single statement inside a code block, wrapping its specific kind.
#[derive(Debug, Clone)]
pub struct CodeBlockStmt {
    /// The actual statement content.
    pub inner: CodeBlockStmtInner,
}

/// An ordered sequence of statements forming a code block (function body,
/// `if`/`else` branch, or loop body).
pub type CodeBlockStmtList = Vec<CodeBlockStmt>;
