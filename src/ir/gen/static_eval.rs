use crate::ast;
use crate::ir::module::IrGenerator;
use crate::ir::Error;

/// Static evaluation methods for the IR generator.
///
/// These functions perform compile-time (static) evaluation of constant expressions
/// from the AST, folding them into concrete `i32` values. This is used for constant
/// folding during IR generation — expressions composed entirely of literals and
/// constant operations can be reduced to a single integer value at compile time.
impl IrGenerator<'_> {
    /// Statically evaluates a right-hand-side value.
    ///
    /// Dispatches to either arithmetic or boolean expression evaluation
    /// depending on the variant of the right value.
    pub fn handle_right_val_static(r: &ast::RightVal) -> Result<i32, Error> {
        match &r.inner {
            ast::RightValInner::ArithExpr(expr) => Self::handle_arith_expr_static(expr),
            ast::RightValInner::BoolExpr(expr) => Self::handle_bool_expr_static(expr),
        }
    }

    /// Statically evaluates an arithmetic expression.
    ///
    /// An arithmetic expression is either a binary operation (e.g., `a + b`)
    /// or a single expression unit (e.g., a literal number or parenthesized expression).
    pub fn handle_arith_expr_static(expr: &ast::ArithExpr) -> Result<i32, Error> {
        match &expr.inner {
            ast::ArithExprInner::ArithBiOpExpr(expr) => Self::handle_arith_biop_expr_static(expr),
            ast::ArithExprInner::ExprUnit(unit) => Self::handle_expr_unit_static(unit),
        }
    }

    /// Statically evaluates a boolean expression.
    ///
    /// A boolean expression is either a binary boolean operation (e.g., `a && b`)
    /// or a single boolean unit (e.g., a comparison or negation).
    pub fn handle_bool_expr_static(expr: &ast::BoolExpr) -> Result<i32, Error> {
        match &expr.inner {
            ast::BoolExprInner::BoolBiOpExpr(expr) => Self::handle_bool_biop_expr_static(expr),
            ast::BoolExprInner::BoolUnit(unit) => Self::handle_bool_unit_static(unit),
        }
    }

    /// Statically evaluates an arithmetic binary operation expression.
    ///
    /// Recursively evaluates both the left and right operands, then applies
    /// the operator (Add, Sub, Mul, Div). Uses checked arithmetic to detect
    /// integer overflow and division by zero, returning appropriate errors.
    pub fn handle_arith_biop_expr_static(expr: &ast::ArithBiOpExpr) -> Result<i32, Error> {
        let left = Self::handle_arith_expr_static(&expr.left)?;
        let right = Self::handle_arith_expr_static(&expr.right)?;
        match &expr.op {
            ast::ArithBiOp::Add => left.checked_add(right).ok_or(Error::IntegerOverflow),
            ast::ArithBiOp::Sub => left.checked_sub(right).ok_or(Error::IntegerOverflow),
            ast::ArithBiOp::Mul => left.checked_mul(right).ok_or(Error::IntegerOverflow),
            ast::ArithBiOp::Div => left.checked_div(right).ok_or(Error::DivisionByZero),
        }
    }

    /// Statically evaluates an expression unit.
    ///
    /// An expression unit can be:
    /// - A numeric literal (`Num`), which is returned directly.
    /// - A parenthesized arithmetic expression, which is evaluated recursively.
    /// - Any other variant (e.g., a variable reference), which cannot be statically
    ///   evaluated and results in an `InvalidExprUnit` error.
    pub fn handle_expr_unit_static(expr: &ast::ExprUnit) -> Result<i32, Error> {
        match &expr.inner {
            ast::ExprUnitInner::Num(num) => Ok(*num),
            ast::ExprUnitInner::ArithExpr(expr) => Self::handle_arith_expr_static(expr),
            _ => Err(Error::InvalidExprUnit {
                expr_unit: expr.clone(),
            }),
        }
    }

    /// Statically evaluates a boolean binary operation expression (AND / OR).
    ///
    /// Evaluates both operands, converts them to booleans (non-zero is true),
    /// and applies the logical AND or OR operator. The result is returned as
    /// an `i32` (1 for true, 0 for false).
    pub fn handle_bool_biop_expr_static(expr: &ast::BoolBiOpExpr) -> Result<i32, Error> {
        let left = Self::handle_bool_expr_static(&expr.left)? != 0;
        let right = Self::handle_bool_expr_static(&expr.right)? != 0;
        if expr.op == ast::BoolBiOp::And {
            Ok((left && right) as i32)
        } else {
            Ok((left || right) as i32)
        }
    }

    /// Statically evaluates a boolean unit.
    ///
    /// A boolean unit can be:
    /// - A comparison expression (e.g., `a < b`).
    /// - A nested boolean expression (parenthesized).
    /// - A unary boolean operation (e.g., `!cond`).
    pub fn handle_bool_unit_static(unit: &ast::BoolUnit) -> Result<i32, Error> {
        match &unit.inner {
            ast::BoolUnitInner::ComExpr(expr) => Self::handle_com_op_expr_static(expr),
            ast::BoolUnitInner::BoolExpr(expr) => Self::handle_bool_expr_static(expr),
            ast::BoolUnitInner::BoolUOpExpr(expr) => Self::handle_bool_uop_expr_static(expr),
        }
    }

    /// Statically evaluates a comparison expression.
    ///
    /// Evaluates both the left and right operands as arithmetic expression units,
    /// then applies the comparison operator (Lt, Eq, Ge, Gt, Le, Ne).
    /// Returns 1 if the comparison is true, 0 otherwise.
    pub fn handle_com_op_expr_static(expr: &ast::ComExpr) -> Result<i32, Error> {
        let left = Self::handle_expr_unit_static(&expr.left)?;
        let right = Self::handle_expr_unit_static(&expr.right)?;
        match expr.op {
            ast::ComOp::Lt => Ok((left < right) as i32),
            ast::ComOp::Eq => Ok((left == right) as i32),
            ast::ComOp::Ge => Ok((left >= right) as i32),
            ast::ComOp::Gt => Ok((left > right) as i32),
            ast::ComOp::Le => Ok((left <= right) as i32),
            ast::ComOp::Ne => Ok((left != right) as i32),
        }
    }

    /// Statically evaluates a boolean unary operation expression.
    ///
    /// Currently only the `Not` operator is supported, which inverts the boolean
    /// value of the inner condition (0 becomes 1, non-zero becomes 0).
    /// For any other unary operator, returns 0 as a default.
    pub fn handle_bool_uop_expr_static(expr: &ast::BoolUOpExpr) -> Result<i32, Error> {
        if expr.op == ast::BoolUOp::Not {
            Ok((Self::handle_bool_unit_static(&expr.cond)? == 0) as i32)
        } else {
            Ok(0)
        }
    }
}
