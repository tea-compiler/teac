//! `Display` trait implementations for all AST node types.
//!
//! Each implementation produces a compact, human-readable textual
//! representation of the corresponding node.  These representations are used
//! when printing error messages, debug output, and the tree-formatted program
//! dump via [`super::tree::DisplayAsTree`].

use super::expr::*;
use super::ops::*;
use super::program::Program;
use super::tree::DisplayAsTree;
use super::types::*;
use std::fmt::{Display, Error, Formatter};

/// Formats a built-in type as its source-level keyword (e.g., `int`).
impl Display for BuiltIn {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            BuiltIn::Int => write!(f, "int"),
        }
    }
}

/// Formats a type-specifier inner node:
/// built-ins use their keyword, composites use their name, and
/// references are wrapped in `&[…]`.
impl Display for TypeSpecifierInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            TypeSpecifierInner::BuiltIn(b) => write!(f, "{}", b),
            TypeSpecifierInner::Composite(name) => write!(f, "{}", name),
            TypeSpecifierInner::Reference(inner) => write!(f, "&[{}]", inner.inner),
        }
    }
}

/// Formats a full type specifier as `<type>@<pos>`, annotating it with its
/// source position for diagnostic purposes.
impl Display for TypeSpecifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}@{}", self.inner, self.pos)
    }
}

/// Formats an arithmetic binary operator as its LLVM IR mnemonic
/// (e.g., `add`, `sub`, `mul`, `sdiv`).
impl Display for ArithBiOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            ArithBiOp::Add => write!(f, "add"),
            ArithBiOp::Sub => write!(f, "sub"),
            ArithBiOp::Mul => write!(f, "mul"),
            ArithBiOp::Div => write!(f, "sdiv"),
        }
    }
}

/// Formats a boolean unary operator as its source-level symbol (`!`).
impl Display for BoolUOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            BoolUOp::Not => write!(f, "!"),
        }
    }
}

/// Formats a boolean binary operator as its source-level symbol
/// (`&&` or `||`).
impl Display for BoolBiOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        let op = match self {
            BoolBiOp::And => "&&",
            BoolBiOp::Or => "||",
        };
        write!(f, "{}", op)
    }
}

/// Formats a comparison operator as its LLVM IR predicate mnemonic
/// (e.g., `eq`, `ne`, `sgt`, …).
impl Display for ComOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            ComOp::Eq => write!(f, "eq"),
            ComOp::Ne => write!(f, "ne"),
            ComOp::Gt => write!(f, "sgt"),
            ComOp::Ge => write!(f, "sge"),
            ComOp::Lt => write!(f, "slt"),
            ComOp::Le => write!(f, "sle"),
        }
    }
}

/// Formats a binary arithmetic expression as `(<left> <op> <right>)`.
impl Display for ArithBiOpExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "({} {} {})", self.left, self.op, self.right)
    }
}

/// Formats the inner part of an arithmetic expression by delegating to
/// the concrete variant.
impl Display for ArithExprInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            ArithExprInner::ArithBiOpExpr(expr) => write!(f, "{}", expr),
            ArithExprInner::ExprUnit(unit) => write!(f, "{}", unit),
        }
    }
}

/// Formats an arithmetic expression by delegating to its inner representation.
impl Display for ArithExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}", self.inner)
    }
}

/// Formats a comparison expression as `(<left> <op> <right>)`.
impl Display for ComExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "({} {} {})", self.left, self.op, self.right)
    }
}

/// Formats a unary boolean expression as `(<op><cond>)`, e.g., `(!x)`.
impl Display for BoolUOpExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "({}{})", self.op, self.cond)
    }
}

/// Formats a binary boolean expression as `(<left> <op> <right>)`.
impl Display for BoolBiOpExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "({} {} {})", self.left, self.op, self.right)
    }
}

/// Formats the inner part of a boolean expression by delegating to the
/// concrete variant.
impl Display for BoolExprInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            BoolExprInner::BoolUnit(b) => write!(f, "{}", b),
            BoolExprInner::BoolBiOpExpr(b) => write!(f, "{}", b),
        }
    }
}

/// Formats a boolean expression by delegating to its inner representation.
impl Display for BoolExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}", self.inner)
    }
}

/// Formats the inner part of a boolean unit by delegating to the concrete
/// variant (comparison, nested boolean expression, or unary not).
impl Display for BoolUnitInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            BoolUnitInner::ComExpr(c) => write!(f, "{}", c),
            BoolUnitInner::BoolExpr(b) => write!(f, "{}", b),
            BoolUnitInner::BoolUOpExpr(u) => write!(f, "{}", u),
        }
    }
}

/// Formats a boolean unit by delegating to its inner representation.
impl Display for BoolUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}", self.inner)
    }
}

/// Formats the inner part of an rvalue by delegating to the concrete variant.
impl Display for RightValInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            RightValInner::ArithExpr(a) => write!(f, "{}", a),
            RightValInner::BoolExpr(b) => write!(f, "{}", b),
        }
    }
}

/// Formats an rvalue by delegating to its inner representation.
impl Display for RightVal {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}", self.inner)
    }
}

/// Formats the inner part of an lvalue.
impl Display for LeftValInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            LeftValInner::Id(id) => write!(f, "{}", id),
            LeftValInner::ArrayExpr(ae) => write!(f, "{}", ae),
            LeftValInner::MemberExpr(me) => write!(f, "{}", me),
        }
    }
}

/// Formats an lvalue by delegating to its inner representation.
impl Display for LeftVal {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}", self.inner)
    }
}

/// Formats an index expression as either a numeric literal or an identifier.
impl Display for IndexExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match &self.inner {
            IndexExprInner::Num(n) => write!(f, "{}", n),
            IndexExprInner::Id(id) => write!(f, "{}", id),
        }
    }
}

/// Formats an array access expression as `<arr>[<idx>]`.
impl Display for ArrayExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}[{}]", self.arr, self.idx)
    }
}

/// Formats a struct member access as `<struct_id>.<member_id>`.
impl Display for MemberExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}.{}", self.struct_id, self.member_id)
    }
}

/// Formats a function call as `<name>(<args>)` or `<module>::<name>(<args>)`
/// for qualified calls.
impl Display for FnCall {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        // Format all argument values as a comma-separated string.
        let args: Vec<String> = self.vals.iter().map(|v| format!("{}", v)).collect();
        if let Some(module) = &self.module_prefix {
            write!(f, "{}::{}({})", module, self.name, args.join(", "))
        } else {
            write!(f, "{}({})", self.name, args.join(", "))
        }
    }
}

/// Formats the inner part of an expression unit.
impl Display for ExprUnitInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            ExprUnitInner::Num(n) => write!(f, "{}", n),
            ExprUnitInner::Id(id) => write!(f, "{}", id),
            ExprUnitInner::ArithExpr(a) => write!(f, "{}", a),
            ExprUnitInner::FnCall(fc) => write!(f, "{}", fc),
            ExprUnitInner::ArrayExpr(ae) => write!(f, "{}", ae),
            ExprUnitInner::MemberExpr(me) => write!(f, "{}", me),
            ExprUnitInner::Reference(id) => write!(f, "&{}", id),
        }
    }
}

/// Formats an expression unit by delegating to its inner representation.
impl Display for ExprUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}", self.inner)
    }
}

/// Formats the entire program using the tree pretty-printer so that
/// `println!("{}", program)` produces a readable AST dump.
impl Display for Program {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        self.fmt_tree_root(f)
    }
}
