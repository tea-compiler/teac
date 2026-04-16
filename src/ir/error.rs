use crate::ast;
use crate::ir::types::Dtype;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Type of variable '{symbol}' has not been determined at point of use")]
    TypeNotDetermined { symbol: String },

    #[error("Cannot assign type '{actual}' to variable '{symbol}' of type '{expected}'")]
    TypeMismatch {
        symbol: String,
        expected: Dtype,
        actual: Dtype,
    },

    #[error("Initialization of structs not supported")]
    StructInitialization,

    #[error("Module '{module_name}' not found: expected header file at '{}'", path.display())]
    ModuleNotFound {
        module_name: String,
        path: PathBuf,
    },

    #[error("Failed to parse module '{module_name}': {message}")]
    ModuleParseFailed {
        module_name: String,
        message: String,
    },

    #[error("Division by zero in constant expression")]
    DivisionByZero,

    #[error("Integer overflow in constant expression")]
    IntegerOverflow,

    #[error("Duplicated definition of variable {symbol}")]
    VariableRedefinition { symbol: String },

    #[error("Conflicted definition of function {symbol}")]
    ConflictedFunction { symbol: String },

    #[error("Mismatched declaration and definition of {symbol}")]
    DeclDefMismatch { symbol: String },

    #[error("Function {symbol} not defined")]
    FunctionNotDefined { symbol: String },

    #[error("Variable {symbol} not defined")]
    VariableNotDefined { symbol: String },

    #[error("Invalid array expression")]
    InvalidArrayExpression,

    #[error("Reference operator '&' can only be applied to array variables, not '{symbol}'")]
    InvalidReference { symbol: String },

    #[error("Array parameter '{symbol}' must be passed by reference: use &[T] instead of [T; N]")]
    ArrayParameterNotAllowed { symbol: String },

    #[error(
        "Array '{symbol}' cannot be used as a value directly; use '&{symbol}' to pass by reference"
    )]
    ArrayUsedAsValue { symbol: String },

    #[error("Invalid struct member expression {expr}")]
    InvalidStructMemberExpression { expr: ast::MemberExpr },

    #[error("Invalid expression unit: {expr_unit}")]
    InvalidExprUnit { expr_unit: ast::ExprUnit },

    #[error("Invalid continue instruction")]
    InvalidContinueInst,

    #[error("Invalid break instruction")]
    InvalidBreakInst,

    #[error("Function '{symbol}' has unsupported return type '{dtype}'")]
    UnsupportedReturnType { symbol: String, dtype: Dtype },

    #[error("Struct type '{member_type}' used in struct '{struct_name}' is not defined")]
    UndefinedStructMemberType {
        struct_name: String,
        member_type: String,
    },

    #[error("I/O error")]
    Io(#[from] std::io::Error),
}
