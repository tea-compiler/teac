//! Expression AST nodes.
//!
//! This module contains all node types that represent expressions in the
//! language: left-hand-side values (lvalues), arithmetic expressions,
//! boolean expressions, comparison expressions, function calls, and the
//! general-purpose expression units that glue everything together.

use super::ops::*;
use super::types::Pos;

/// An lvalue — a memory location that can appear on the left side of an
/// assignment.
#[derive(Debug, Clone)]
pub struct LeftVal {
    /// Source position of this lvalue.
    pub pos: Pos,
    /// The kind of lvalue (simple identifier, array element, or struct member).
    pub inner: LeftValInner,
}

/// The inner representation of an lvalue.
#[derive(Debug, Clone)]
pub enum LeftValInner {
    /// A simple variable name.
    Id(String),
    /// An array element access, e.g. `a[i]`.
    ArrayExpr(Box<ArrayExpr>),
    /// A struct member access, e.g. `s.field`.
    MemberExpr(Box<MemberExpr>),
}

/// The inner representation of an array index expression.
#[derive(Debug, Clone)]
pub enum IndexExprInner {
    /// A literal numeric index, e.g. `a[2]`.
    Num(usize),
    /// An identifier used as an index, e.g. `a[i]`.
    Id(String),
}

/// An index expression used inside an array access.
#[derive(Debug, Clone)]
pub struct IndexExpr {
    /// Whether the index is a literal number or a variable name.
    pub inner: IndexExprInner,
}

/// An array element access expression, e.g. `arr[idx]`.
#[derive(Debug, Clone)]
pub struct ArrayExpr {
    /// The array being indexed (itself an lvalue, enabling `a[i][j]`).
    pub arr: Box<LeftVal>,
    /// The index expression.
    pub idx: Box<IndexExpr>,
}

/// A struct member access expression, e.g. `obj.field`.
#[derive(Debug, Clone)]
pub struct MemberExpr {
    /// The struct lvalue being accessed.
    pub struct_id: Box<LeftVal>,
    /// The name of the member field.
    pub member_id: String,
}

/// A binary arithmetic expression, e.g. `a + b`.
#[derive(Debug, Clone)]
pub struct ArithBiOpExpr {
    /// The arithmetic operator.
    pub op: ArithBiOp,
    /// The left operand.
    pub left: Box<ArithExpr>,
    /// The right operand.
    pub right: Box<ArithExpr>,
}

/// The inner representation of an arithmetic expression.
#[derive(Debug, Clone)]
pub enum ArithExprInner {
    /// A binary arithmetic operation such as `a + b`.
    ArithBiOpExpr(Box<ArithBiOpExpr>),
    /// A leaf expression unit (number literal, identifier, function call, …).
    ExprUnit(Box<ExprUnit>),
}

/// An arithmetic expression, pairing the inner value with a source position.
#[derive(Debug, Clone)]
pub struct ArithExpr {
    /// Source position of this expression.
    pub pos: Pos,
    /// The actual arithmetic expression content.
    pub inner: ArithExprInner,
}

/// A comparison expression that yields a boolean, e.g. `a < b`.
#[derive(Debug, Clone)]
pub struct ComExpr {
    /// The comparison operator.
    pub op: ComOp,
    /// The left operand (must be an expression unit).
    pub left: Box<ExprUnit>,
    /// The right operand (must be an expression unit).
    pub right: Box<ExprUnit>,
}

/// A unary boolean expression, e.g. `!cond`.
#[derive(Debug, Clone)]
pub struct BoolUOpExpr {
    /// The boolean unary operator (currently only `Not`).
    pub op: BoolUOp,
    /// The operand boolean unit to negate.
    pub cond: Box<BoolUnit>,
}

/// A binary boolean expression, e.g. `a && b`.
#[derive(Debug, Clone)]
pub struct BoolBiOpExpr {
    /// The boolean binary operator (`And` or `Or`).
    pub op: BoolBiOp,
    /// The left operand.
    pub left: Box<BoolExpr>,
    /// The right operand.
    pub right: Box<BoolExpr>,
}

/// The inner representation of a boolean expression.
#[derive(Debug, Clone)]
pub enum BoolExprInner {
    /// A binary boolean operation such as `a && b`.
    BoolBiOpExpr(Box<BoolBiOpExpr>),
    /// A leaf boolean unit (comparison, nested bool expr, or unary not).
    BoolUnit(Box<BoolUnit>),
}

/// A boolean expression, pairing the inner value with a source position.
#[derive(Debug, Clone)]
pub struct BoolExpr {
    /// Source position of this boolean expression.
    pub pos: Pos,
    /// The actual boolean expression content.
    pub inner: BoolExprInner,
}

/// The inner representation of a boolean unit — the atomic building block
/// from which boolean expressions are composed.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum BoolUnitInner {
    /// A comparison expression, e.g. `a == b`.
    ComExpr(Box<ComExpr>),
    /// A parenthesised or nested boolean expression.
    BoolExpr(Box<BoolExpr>),
    /// A unary boolean expression, e.g. `!cond`.
    BoolUOpExpr(Box<BoolUOpExpr>),
}

/// A boolean unit with its source position.
#[derive(Debug, Clone)]
pub struct BoolUnit {
    /// Source position of this boolean unit.
    pub pos: Pos,
    /// The actual boolean unit content.
    pub inner: BoolUnitInner,
}

/// A function call expression, e.g. `foo(a, b)` or `mod::foo(a, b)`.
#[derive(Debug, Clone)]
pub struct FnCall {
    /// Optional module prefix for qualified calls such as `io::print`.
    pub module_prefix: Option<String>,
    /// The unqualified function name.
    pub name: String,
    /// The list of argument values passed to the function.
    pub vals: RightValList,
}

/// Implementation of helper methods for function calls.
impl FnCall {
    /// Returns the fully-qualified function name, including the module prefix
    /// if one is present (e.g., `"io::print"`), or just the bare function
    /// name otherwise (e.g., `"print"`).
    pub fn qualified_name(&self) -> String {
        if let Some(module) = &self.module_prefix {
            // Combine module prefix and function name with `::` separator.
            format!("{module}::{}", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// The inner representation of a leaf expression unit.
#[derive(Debug, Clone)]
pub enum ExprUnitInner {
    /// An integer literal.
    Num(i32),
    /// A simple variable identifier.
    Id(String),
    /// A parenthesised arithmetic sub-expression.
    ArithExpr(Box<ArithExpr>),
    /// A function call whose return value is used as a value.
    FnCall(Box<FnCall>),
    /// An array element access used as a value.
    ArrayExpr(Box<ArrayExpr>),
    /// A struct member access used as a value.
    MemberExpr(Box<MemberExpr>),
    /// A reference to a variable, e.g. `&x`.
    Reference(String),
}

/// An expression unit — the leaf node of arithmetic expressions — paired
/// with a source position.
#[derive(Debug, Clone)]
pub struct ExprUnit {
    /// Source position of this expression unit.
    pub pos: Pos,
    /// The actual expression unit content.
    pub inner: ExprUnitInner,
}

/// The inner representation of a right-hand-side value.
#[derive(Debug, Clone)]
pub enum RightValInner {
    /// An arithmetic expression used as an rvalue.
    ArithExpr(Box<ArithExpr>),
    /// A boolean expression used as an rvalue.
    BoolExpr(Box<BoolExpr>),
}

/// An rvalue — any value that can appear on the right side of an assignment
/// or as a function argument.
#[derive(Debug, Clone)]
pub struct RightVal {
    /// The actual rvalue content (arithmetic or boolean).
    pub inner: RightValInner,
}

/// A list of right-hand-side values, used for function argument lists.
pub type RightValList = Vec<RightVal>;
