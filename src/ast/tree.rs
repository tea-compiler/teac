use super::decl::*;
use super::expr::*;
use super::program::*;
use super::stmt::*;
use std::fmt::{Error, Formatter};

pub trait DisplayAsTree {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error>;

    fn fmt_tree_root(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        self.fmt_tree(f, &[], true)
    }
}

fn tree_indent(indent_levels: &[bool], is_last: bool) -> String {
    let mut s = String::new();
    for &last in indent_levels.iter() {
        if last {
            s.push_str("   ");
        } else {
            s.push_str("│  ");
        }
    }
    if is_last {
        s.push_str("└─");
    } else {
        s.push_str("├─");
    }
    s
}

impl DisplayAsTree for Program {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}Program", tree_indent(indent_levels, is_last))?;
        let mut new_indent = indent_levels.to_vec();
        new_indent.push(!is_last);
        let last_index = self.elements.len().saturating_sub(1);
        for (i, elem) in self.elements.iter().enumerate() {
            elem.fmt_tree(f, &new_indent, i == last_index)?;
        }
        Ok(())
    }
}

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

impl DisplayAsTree for VarDeclStmt {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        writeln!(f, "{}VarDeclStmt", tree_indent(indent_levels, is_last))?;
        self.inner.fmt_tree(f, indent_levels, true)
    }
}

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

impl DisplayAsTree for VarDecl {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
        let type_str = self
            .type_specifier
            .as_ref()
            .clone()
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
            let mut new_indent = indent_levels.to_vec();
            new_indent.push(!is_last);
            writeln!(f, "{}Params:", tree_indent(&new_indent, false))?;
            params.decls.fmt_tree(f, &new_indent, true)?;
        }
        Ok(())
    }
}

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
                    writeln!(f, "{}{} = {:?}", prefix, self.identifier, vals)
                }
                ArrayInitializer::Fill { val, count } => {
                    writeln!(f, "{}{} = [{}; {}]", prefix, self.identifier, val, count)
                }
            },
        }
    }
}

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

        self.left_val.fmt_tree(f, &new_indent, false)?;
        self.right_val.fmt_tree(f, &new_indent, true)
    }
}

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
        writeln!(f, "{}IfBranch:", tree_indent(&new_indent, false))?;
        self.if_stmts.fmt_tree(f, &new_indent, true)?;
        if let Some(e) = &self.else_stmts {
            writeln!(f, "{}ElseBranch:", tree_indent(&new_indent, false))?;
            e.fmt_tree(f, &new_indent, true)?;
        }
        Ok(())
    }
}

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
        self.arr.fmt_tree(f, &new_indent, false)?;
        self.idx.fmt_tree(f, &new_indent, true)
    }
}

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
        self.left.fmt_tree(f, &new_indent, false)?;
        self.right.fmt_tree(f, &new_indent, true)
    }
}

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
        self.left.fmt_tree(f, &new_indent, false)?;
        self.right.fmt_tree(f, &new_indent, true)
    }
}

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

impl DisplayAsTree for FnCall {
    fn fmt_tree(
        &self,
        f: &mut Formatter<'_>,
        indent_levels: &[bool],
        is_last: bool,
    ) -> Result<(), Error> {
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

        self.left.fmt_tree(f, &new_indent, false)?;
        self.right.fmt_tree(f, &new_indent, true)
    }
}

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
