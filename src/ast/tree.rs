//! Tree pretty-printer for the AST.
//!
//! This module defines the [`DisplayAsTree`] trait and provides implementations
//! for every AST node type.  When a node is printed with this trait it
//! produces an indented, Unicode-box-drawing tree that mirrors the logical
//! structure of the AST, making it easy to read program structure at a glance.
//!
//! The indentation state is passed down through the `indent_levels` slice.
//! Each element records whether the corresponding ancestor was the *last*
//! child at its level; this drives the choice between `│  ` (more siblings
//! follow) and `   ` (no more siblings) connector strings.

use super::decl::*;
use super::expr::*;
use super::program::*;
use super::stmt::*;
use std::fmt::{Error, Formatter};

/// Trait for formatting an AST node as an indented tree.
pub trait DisplayAsTree {
    /// Write this node (and all its children) to `f` as an indented tree.
    ///
    /// * `indent_levels` – a slice whose length equals the current nesting
    ///   depth; each `bool` records whether the corresponding ancestor was
    ///   the last child at its level (`true` → last, so print spaces instead
    ///   of a vertical bar).
    /// * `is_last` – whether *this* node is the last sibling among its
    ///   parent's children.
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error>;

    /// Convenience method that starts a fresh tree with no indentation.
    /// Equivalent to calling `fmt_tree(f, &[], true)`.
    fn fmt_tree_root(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        self.fmt_tree(f, &[], true)
    }
}

/// Builds the indentation prefix string for a tree node.
///
/// For each ancestor level, appends either `"   "` (if that ancestor was the
/// last child, so no vertical bar is needed) or `"│  "` (if more siblings
/// follow at that level).  Finally appends `"└─"` for the last child or
/// `"├─"` for any other child.
fn tree_indent(indent_levels: &[bool], is_last: bool) -> String {
    let mut s = String::new();
    for &last in indent_levels.iter() {
        if last {
            // Ancestor was the last child — no vertical connector needed.
            s.push_str("   ");
        } else {
            // More siblings exist at this ancestor level — draw vertical bar.
            s.push_str("│  ");
        }
    }
    if is_last {
        // This node is the last child — use a corner connector.
        s.push_str("└─");
    } else {
        // More siblings follow — use a tee connector.
        s.push_str("├─");
    }
    s
}

/// Formats the root `Program` node, listing every top-level element as a
/// child in the tree.
impl DisplayAsTree for Program {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}Program", tree_indent(indent_levels, is_last))?;
        // Build the indentation context for children.
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(!is_last);
        let last_index = self.elements.len().saturating_sub(1);
        for (i, elem) in self.elements.iter().enumerate() {
            elem.fmt_tree(f, &new_indent, i == last_index)?;
        }
        Ok(())
    }
}

/// Delegates formatting to the concrete element variant.
impl DisplayAsTree for ProgramElement {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        match &self.inner {
            ProgramElementInner::VarDeclStmt(v) => v.fmt_tree(f, indent_levels, is_last),
            ProgramElementInner::StructDef(s) => s.fmt_tree(f, indent_levels, is_last),
            ProgramElementInner::FnDeclStmt(d) => d.fmt_tree(f, indent_levels, is_last),
            ProgramElementInner::FnDef(def) => def.fmt_tree(f, indent_levels, is_last),
        }
    }
}

/// Prints a `VarDeclStmt` header then delegates to the inner decl/def.
impl DisplayAsTree for VarDeclStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}VarDeclStmt", tree_indent(indent_levels, is_last))?;
        // The inner node is always the single (last) child.
        self.inner.fmt_tree(f, indent_levels, true)
    }
}

/// Delegates to either the `Decl` or `Def` variant of a variable
/// declaration statement.
impl DisplayAsTree for VarDeclStmtInner {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        match self {
            VarDeclStmtInner::Decl(v) => v.fmt_tree(f, indent_levels, is_last),
            VarDeclStmtInner::Def(d) => d.fmt_tree(f, indent_levels, is_last),
        }
    }
}

/// Prints a single variable declaration as `<name>: <type>`, using
/// `"unknown"` when no type annotation is present.
impl DisplayAsTree for VarDecl {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        // Render the type specifier, falling back to "unknown" if absent.
        let type_str = self
            .type_specifier
            .as_ref()
            .map_or("unknown".to_string(), |ts| ts.to_string());
        writeln!(
            f,
            "{}{}: {}",
            tree_indent(indent_levels, is_last),
            self.identifier,
            type_str
        )
    }
}

/// Transparent forwarding implementation: delegates directly to the
/// pointed-to value so that `Box<T>` nodes behave identically to `T`.
impl<T: DisplayAsTree + ?Sized> DisplayAsTree for Box<T> {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        (**self).fmt_tree(f, indent_levels, is_last)
    }
}

/// Skips rendering entirely when the `Option` is `None`; otherwise
/// delegates to the inner boxed value.
impl<T: DisplayAsTree + ?Sized> DisplayAsTree for Option<Box<T>> {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        if let Some(v) = self {
            v.fmt_tree(f, indent_levels, is_last)
        } else {
            Ok(())
        }
    }
}

/// Prints the function name and, optionally, an indented `Params:` subtree
/// listing all parameter declarations.
impl DisplayAsTree for FnDecl {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}FnDecl {}",
            tree_indent(indent_levels, is_last),
            self.identifier
        )?;
        if let Some(params) = &self.param_decl {
            // Extend the indentation context for the parameter subtree.
            let mut new_indent = indent_levels.to_vec();
            new_indent.push(!is_last);
            writeln!(f, "{}Params:", tree_indent(&new_indent, false))?;
            params.decls.fmt_tree(f, &new_indent, true)?;
        }
        Ok(())
    }
}

/// Delegates directly to the inner `FnDecl`.
impl DisplayAsTree for FnDeclStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        self.fn_decl.fmt_tree(f, indent_levels, is_last)
    }
}

/// Prints the function name then lists the body statements as children.
impl DisplayAsTree for FnDef {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}FnDef {}",
            tree_indent(indent_levels, is_last),
            self.fn_decl.identifier
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(!is_last);
        self.stmts.fmt_tree(f, &new_indent, true)
    }
}

/// Prints a variable definition as either a scalar assignment
/// (`name = val`) or an array initializer.
impl DisplayAsTree for VarDef {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        let prefix = tree_indent(indent_levels, is_last);
        match &self.inner {
            VarDefInner::Scalar(s) => writeln!(f, "{}{} = {}", prefix, self.identifier, s.val),
            VarDefInner::Array(a) => match &a.initializer {
                ArrayInitializer::ExplicitList(vals) => {
                    // Print the debug representation of all explicit values.
                    writeln!(f, "{}{} = {:?}", prefix, self.identifier, vals)
                }
                ArrayInitializer::Fill { val, count } => {
                    // Print the fill syntax: `name = [val; count]`.
                    writeln!(f, "{}{} = [{}; {}]", prefix, self.identifier, val, count)
                }
            },
        }
    }
}

/// Prints a `VarDeclList` header then lists every declaration as a child.
impl DisplayAsTree for VarDeclList {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}VarDeclList", tree_indent(indent_levels, is_last))?;

        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        let last_index = self.len().saturating_sub(1);
        for (i, decl) in self.iter().enumerate() {
            decl.fmt_tree(f, &new_indent, i == last_index)?;
        }
        Ok(())
    }
}

/// Prints an `AssignmentStmt` header then shows the lvalue and rvalue as
/// the two children.
impl DisplayAsTree for AssignmentStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}AssignmentStmt", tree_indent(indent_levels, is_last))?;

        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        // lvalue is not the last child; rvalue is.
        self.left_val.fmt_tree(f, &new_indent, false)?;
        self.right_val.fmt_tree(f, &new_indent, true)
    }
}

/// Prints `CallStmt <fn_name>` then lists each argument as a child.
impl DisplayAsTree for CallStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}CallStmt {}",
            tree_indent(indent_levels, is_last),
            self.fn_call.name
        )?;

        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        let last_index = self.fn_call.vals.len().saturating_sub(1);
        for (i, val) in self.fn_call.vals.iter().enumerate() {
            val.fmt_tree(f, &new_indent, i == last_index)?;
        }
        Ok(())
    }
}

/// Delegates to the concrete statement variant inside a code block.
impl DisplayAsTree for CodeBlockStmtInner {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        match self {
            CodeBlockStmtInner::VarDecl(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::Assignment(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::Call(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::If(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::While(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::Return(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::Continue(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::Break(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
            CodeBlockStmtInner::Null(stmt) => stmt.fmt_tree(f, indent_levels, is_last),
        }
    }
}

/// Delegates to the inner statement kind.
impl DisplayAsTree for CodeBlockStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        self.inner.fmt_tree(f, indent_levels, is_last)
    }
}

/// Prints every statement in the list at the same indentation level,
/// marking only the final element as `is_last`.
impl DisplayAsTree for CodeBlockStmtList {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        _is_last: bool,
    ) -> Result<(), Error> {
        let last_index = self.len().saturating_sub(1);
        for (i, stmt) in self.iter().enumerate() {
            stmt.fmt_tree(f, indent_levels, i == last_index)?;
        }
        Ok(())
    }
}

/// Prints `IfStmt Cond: <cond>` then shows the `IfBranch` and optional
/// `ElseBranch` subtrees.
impl DisplayAsTree for IfStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}IfStmt Cond: {}",
            tree_indent(indent_levels, is_last),
            self.bool_unit
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        // Print the then-branch header; it is not the last child when an
        // else-branch also exists.
        writeln!(f, "{}IfBranch:", tree_indent(&new_indent, false))?;
        self.if_stmts.fmt_tree(f, &new_indent, true)?;
        if let Some(e) = &self.else_stmts {
            writeln!(f, "{}ElseBranch:", tree_indent(&new_indent, false))?;
            e.fmt_tree(f, &new_indent, true)?;
        }
        Ok(())
    }
}

/// Prints `WhileStmt Cond: <cond>` then shows the loop `Body` subtree.
impl DisplayAsTree for WhileStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}WhileStmt Cond: {}",
            tree_indent(indent_levels, is_last),
            self.bool_unit
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        writeln!(f, "{}Body:", tree_indent(&new_indent, false))?;
        self.stmts.fmt_tree(f, &new_indent, true)
    }
}

/// Prints `ReturnStmt <val>` if a value is returned, or just `ReturnStmt`
/// for a void return.
impl DisplayAsTree for ReturnStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        if let Some(v) = &self.val {
            writeln!(f, "{}ReturnStmt {}", tree_indent(indent_levels, is_last), v)
        } else {
            writeln!(f, "{}ReturnStmt", tree_indent(indent_levels, is_last))
        }
    }
}

/// Prints a leaf `ContinueStmt` node.
impl DisplayAsTree for ContinueStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}ContinueStmt", tree_indent(indent_levels, is_last))
    }
}

/// Prints a leaf `BreakStmt` node.
impl DisplayAsTree for BreakStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}BreakStmt", tree_indent(indent_levels, is_last))
    }
}

/// Prints a leaf `NullStmt` node (empty statement).
impl DisplayAsTree for NullStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}NullStmt", tree_indent(indent_levels, is_last))
    }
}

/// Prints a `LeftVal` header then its inner variant (identifier, array
/// access, or member access) as the single child.
impl DisplayAsTree for LeftVal {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}LeftVal", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        match &self.inner {
            LeftValInner::Id(id) => writeln!(f, "{}Id {}", tree_indent(&new_indent, true), id),
            LeftValInner::ArrayExpr(ae) => ae.fmt_tree(f, &new_indent, true),
            LeftValInner::MemberExpr(me) => me.fmt_tree(f, &new_indent, true),
        }
    }
}

/// Prints a `RightVal` header then its inner variant (arithmetic or boolean
/// expression) as the single child.
impl DisplayAsTree for RightVal {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}RightVal", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        match &self.inner {
            RightValInner::ArithExpr(ae) => ae.fmt_tree(f, &new_indent, true),
            RightValInner::BoolExpr(be) => be.fmt_tree(f, &new_indent, true),
        }
    }
}

/// Prints `StructDef <name>` then lists field declarations as children.
impl DisplayAsTree for StructDef {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}StructDef {}",
            tree_indent(indent_levels, is_last),
            self.identifier
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        self.decls.fmt_tree(f, &new_indent, true)
    }
}

/// Prints an `ArrayExpr` header then shows the array lvalue and index
/// expression as two children.
impl DisplayAsTree for ArrayExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}ArrayExpr", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        // Array lvalue is not the last child; index expression is.
        self.arr.fmt_tree(f, &new_indent, false)?;
        self.idx.fmt_tree(f, &new_indent, true)
    }
}

/// Prints `MemberExpr <field>` then shows the base struct lvalue as the
/// single child.
impl DisplayAsTree for MemberExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}MemberExpr {}",
            tree_indent(indent_levels, is_last),
            self.member_id
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        self.struct_id.fmt_tree(f, &new_indent, true)
    }
}

/// Prints an `ArithExpr` header then delegates to the concrete inner variant
/// (binary operation or expression unit).
impl DisplayAsTree for ArithExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}ArithExpr", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        match &self.inner {
            ArithExprInner::ArithBiOpExpr(expr) => expr.fmt_tree(f, &new_indent, true),
            ArithExprInner::ExprUnit(unit) => unit.fmt_tree(f, &new_indent, true),
        }
    }
}

/// Prints a `BoolExpr` header then delegates to the concrete inner variant
/// (binary boolean operation or boolean unit).
impl DisplayAsTree for BoolExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}BoolExpr", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        match &self.inner {
            BoolExprInner::BoolBiOpExpr(expr) => expr.fmt_tree(f, &new_indent, true),
            BoolExprInner::BoolUnit(unit) => unit.fmt_tree(f, &new_indent, true),
        }
    }
}

/// Prints an index expression as either `IndexExpr Num(<n>)` or
/// `IndexExpr Id(<name>)`.
impl DisplayAsTree for IndexExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        match &self.inner {
            IndexExprInner::Num(n) => writeln!(
                f,
                "{}IndexExpr Num({})",
                tree_indent(indent_levels, is_last),
                n
            ),
            IndexExprInner::Id(s) => writeln!(
                f,
                "{}IndexExpr Id({})",
                tree_indent(indent_levels, is_last),
                s
            ),
        }
    }
}

/// Prints `ArithBiOpExpr <op>` then shows the left and right operands as
/// two children.
impl DisplayAsTree for ArithBiOpExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}ArithBiOpExpr {:?}",
            tree_indent(indent_levels, is_last),
            self.op
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        // Left operand is not the last child; right operand is.
        self.left.fmt_tree(f, &new_indent, false)?;
        self.right.fmt_tree(f, &new_indent, true)
    }
}

/// Prints an `ExprUnit` header then delegates to the concrete inner variant
/// (number, identifier, sub-expression, function call, array access, member
/// access, or reference).
impl DisplayAsTree for ExprUnit {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}ExprUnit", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        match &self.inner {
            ExprUnitInner::Num(n) => writeln!(f, "{}Num({})", tree_indent(&new_indent, true), n),
            ExprUnitInner::Id(id) => writeln!(f, "{}Id({})", tree_indent(&new_indent, true), id),
            ExprUnitInner::ArithExpr(ae) => ae.fmt_tree(f, &new_indent, true),
            ExprUnitInner::FnCall(fc) => fc.fmt_tree(f, &new_indent, true),
            ExprUnitInner::ArrayExpr(ae) => ae.fmt_tree(f, &new_indent, true),
            ExprUnitInner::MemberExpr(me) => me.fmt_tree(f, &new_indent, true),
            ExprUnitInner::Reference(id) => {
                writeln!(f, "{}Ref({})", tree_indent(&new_indent, true), id)
            }
        }
    }
}

/// Prints `BoolBiOpExpr <op>` then shows the left and right operands as
/// two children.
impl DisplayAsTree for BoolBiOpExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}BoolBiOpExpr {:?}",
            tree_indent(indent_levels, is_last),
            self.op
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        // Left operand is not the last child; right operand is.
        self.left.fmt_tree(f, &new_indent, false)?;
        self.right.fmt_tree(f, &new_indent, true)
    }
}

/// Prints a `BoolUnit` header then delegates to the concrete inner variant
/// (comparison, nested boolean expression, or unary boolean operation).
impl DisplayAsTree for BoolUnit {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}BoolUnit", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);
        match &self.inner {
            BoolUnitInner::ComExpr(c) => c.fmt_tree(f, &new_indent, true),
            BoolUnitInner::BoolExpr(b) => b.fmt_tree(f, &new_indent, true),
            BoolUnitInner::BoolUOpExpr(u) => u.fmt_tree(f, &new_indent, true),
        }
    }
}

/// Prints `FnCall: <qualified_name>` then lists each argument as a child.
impl DisplayAsTree for FnCall {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        // Use the qualified name so that module-prefixed calls are shown correctly.
        let fn_name = self.qualified_name();
        writeln!(
            f,
            "{}FnCall: {}",
            tree_indent(indent_levels, is_last),
            fn_name
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        let last_index = self.vals.len().saturating_sub(1);
        for (i, val) in self.vals.iter().enumerate() {
            val.fmt_tree(f, &new_indent, i == last_index)?;
        }

        Ok(())
    }
}

/// Prints a `ComExpr` header then shows the left and right operands as
/// two children.
impl DisplayAsTree for ComExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}ComExpr", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        // Left operand is not the last child; right operand is.
        self.left.fmt_tree(f, &new_indent, false)?;
        self.right.fmt_tree(f, &new_indent, true)
    }
}

/// Prints `BoolUOpExpr <op>` then shows the operand condition as the single
/// child.
impl DisplayAsTree for BoolUOpExpr {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(
            f,
            "{}BoolUOpExpr {:?}",
            tree_indent(indent_levels, is_last),
            self.op
        )?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(is_last);

        self.cond.fmt_tree(f, &new_indent, true)
    }
}
