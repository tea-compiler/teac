//! Operator enumerations used in arithmetic, boolean, and comparison expressions.
//!
//! Each operator kind is represented as its own enum so that the type system
//! enforces that, for example, a boolean unary operator can never be used
//! where an arithmetic binary operator is expected.

/// Arithmetic binary operators.
#[derive(Debug, Clone)]
pub enum ArithBiOp {
    /// Addition (`+`).
    Add,
    /// Subtraction (`-`).
    Sub,
    /// Multiplication (`*`).
    Mul,
    /// Division (`/`), mapped to signed integer division in codegen.
    Div,
}

/// Boolean unary operators.
#[derive(Debug, PartialEq, Clone)]
pub enum BoolUOp {
    /// Logical negation (`!`).
    Not,
}

/// Boolean binary operators.
#[derive(Debug, PartialEq, Clone)]
pub enum BoolBiOp {
    /// Logical conjunction (`&&`).
    And,
    /// Logical disjunction (`||`).
    Or,
}

/// Comparison operators used to produce boolean results from two values.
#[derive(Debug, Clone)]
pub enum ComOp {
    /// Strictly less than (`<`).
    Lt,
    /// Less than or equal (`<=`).
    Le,
    /// Strictly greater than (`>`).
    Gt,
    /// Greater than or equal (`>=`).
    Ge,
    /// Equal (`==`).
    Eq,
    /// Not equal (`!=`).
    Ne,
}
