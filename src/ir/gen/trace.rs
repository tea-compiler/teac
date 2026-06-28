use crate::ast;
use crate::ir::function::FunctionGenerator;
use crate::ir::module::IrGenerator;
use crate::ir::stmt::{ArithBinOp, CmpPredicate};
use crate::ir::types::{Dtype, FunctionType};
use crate::ir::value::{Local, Operand};
use crate::ir::Function;
use std::collections::{HashMap, HashSet};

pub(super) struct TracePlan {
    pub functions: HashSet<String>,
    pub needs_runtime: bool,
}

pub(super) fn plan(program: &ast::Program) -> TracePlan {
    let mut graph = HashMap::new();
    let mut roots = HashSet::new();
    let mut defined = HashSet::new();

    for elem in &program.elements {
        if let ast::ProgramElementInner::FnDef(fn_def) = &elem.inner {
            let name = fn_def.fn_decl.identifier.clone();
            let sites = collect_call_sites(&fn_def.stmts);
            defined.insert(name.clone());
            roots.extend(sites.roots);
            graph.insert(name, sites.calls);
        }
    }

    let needs_runtime = !roots.is_empty();
    let mut traced = HashSet::new();
    let mut worklist = roots.into_iter().collect::<Vec<_>>();
    while let Some(function) = worklist.pop() {
        if defined.contains(&function) && traced.insert(function.clone()) {
            for call in &graph[&function] {
                worklist.push(call.clone());
            }
        }
    }

    TracePlan {
        functions: traced,
        needs_runtime,
    }
}

pub(super) fn register_runtime(gen: &mut IrGenerator<'_>) {
    let fns = [
        ("__teac_trace_begin", Dtype::Void, vec![]),
        ("__teac_trace_finish", Dtype::Void, vec![]),
        ("__teac_trace_enter", Dtype::Void, vec![]),
        ("__teac_trace_leave", Dtype::Void, vec![]),
        ("__teac_trace_event", Dtype::Void, vec![Dtype::I32]),
        ("__teac_trace_putch", Dtype::Void, vec![Dtype::I32]),
        ("__teac_trace_putint", Dtype::Void, vec![Dtype::I32]),
        ("__teac_trace_putbool", Dtype::Void, vec![Dtype::I32]),
    ];

    for (name, return_dtype, args) in fns {
        gen.registry.function_types.insert(
            name.to_string(),
            FunctionType {
                return_dtype,
                arguments: args
                    .into_iter()
                    .enumerate()
                    .map(|(i, dtype)| (format!("arg{i}"), dtype))
                    .collect(),
            },
        );
        gen.registry
            .link_names
            .insert(name.to_string(), name.to_string());
        gen.module.function_list.insert(
            name.to_string(),
            Function {
                identifier: name.to_string(),
                link_name: name.to_string(),
                body: None,
            },
        );
    }
}

#[derive(Default)]
struct CallSites {
    calls: HashSet<String>,
    roots: HashSet<String>,
}

fn collect_call_sites(stmts: &[ast::CodeBlockStmt]) -> CallSites {
    let mut sites = CallSites::default();
    collect_stmts(stmts, &mut sites);
    sites
}

fn collect_stmts(stmts: &[ast::CodeBlockStmt], sites: &mut CallSites) {
    for stmt in stmts {
        match &stmt.inner {
            ast::CodeBlockStmtInner::VarDecl(s) => {
                if let ast::VarDeclStmtInner::Def(def) = &s.inner {
                    collect_var_def(def, sites);
                }
            }
            ast::CodeBlockStmtInner::Assignment(s) => collect_right_val(&s.right_val, sites),
            ast::CodeBlockStmtInner::Call(s) => collect_fn_call(&s.fn_call, sites),
            ast::CodeBlockStmtInner::Trace(s) => collect_trace_call(&s.fn_call, sites),
            ast::CodeBlockStmtInner::If(s) => {
                collect_bool_unit(&s.bool_unit, sites);
                collect_stmts(&s.if_stmts, sites);
                if let Some(else_stmts) = &s.else_stmts {
                    collect_stmts(else_stmts, sites);
                }
            }
            ast::CodeBlockStmtInner::While(s) => {
                collect_bool_unit(&s.bool_unit, sites);
                collect_stmts(&s.stmts, sites);
            }
            ast::CodeBlockStmtInner::Return(s) => {
                if let Some(val) = &s.val {
                    collect_right_val(val, sites);
                }
            }
            ast::CodeBlockStmtInner::Continue(_)
            | ast::CodeBlockStmtInner::Break(_)
            | ast::CodeBlockStmtInner::Null(_) => {}
        }
    }
}

fn collect_var_def(def: &ast::VarDef, sites: &mut CallSites) {
    match &def.inner {
        ast::VarDefInner::Scalar(scalar) => collect_right_val(&scalar.val, sites),
        ast::VarDefInner::Array(array) => match &array.initializer {
            ast::ArrayInitializer::ExplicitList(vals) => {
                for val in vals {
                    collect_right_val(val, sites);
                }
            }
            ast::ArrayInitializer::Fill { val, .. } => collect_right_val(val, sites),
        },
    }
}

fn collect_fn_call(call: &ast::FnCall, sites: &mut CallSites) {
    sites.calls.insert(call.qualified_name());
    for arg in &call.vals {
        collect_right_val(arg, sites);
    }
}

fn collect_trace_call(call: &ast::FnCall, sites: &mut CallSites) {
    sites.roots.insert(call.qualified_name());
    collect_fn_call(call, sites);
}

fn collect_expr_unit(unit: &ast::ExprUnit, sites: &mut CallSites) {
    match &unit.inner {
        ast::ExprUnitInner::ArithExpr(expr) => collect_arith_expr(expr, sites),
        ast::ExprUnitInner::FnCall(call) => collect_fn_call(call, sites),
        ast::ExprUnitInner::TraceCall(call) => collect_trace_call(call, sites),
        ast::ExprUnitInner::ArrayExpr(_)
        | ast::ExprUnitInner::MemberExpr(_)
        | ast::ExprUnitInner::Num(_)
        | ast::ExprUnitInner::Id(_)
        | ast::ExprUnitInner::Reference(_) => {}
    }
}

fn collect_arith_expr(expr: &ast::ArithExpr, sites: &mut CallSites) {
    match &expr.inner {
        ast::ArithExprInner::ArithBiOpExpr(expr) => {
            collect_arith_expr(&expr.left, sites);
            collect_arith_expr(&expr.right, sites);
        }
        ast::ArithExprInner::ExprUnit(unit) => collect_expr_unit(unit, sites),
    }
}

fn collect_bool_unit(unit: &ast::BoolUnit, sites: &mut CallSites) {
    match &unit.inner {
        ast::BoolUnitInner::ComExpr(expr) => {
            collect_expr_unit(&expr.left, sites);
            collect_expr_unit(&expr.right, sites);
        }
        ast::BoolUnitInner::BoolExpr(expr) => collect_bool_expr(expr, sites),
        ast::BoolUnitInner::BoolUOpExpr(expr) => collect_bool_unit(&expr.cond, sites),
    }
}

fn collect_bool_expr(expr: &ast::BoolExpr, sites: &mut CallSites) {
    match &expr.inner {
        ast::BoolExprInner::BoolBiOpExpr(expr) => {
            collect_bool_expr(&expr.left, sites);
            collect_bool_expr(&expr.right, sites);
        }
        ast::BoolExprInner::BoolUnit(unit) => collect_bool_unit(unit, sites),
    }
}

fn collect_right_val(val: &ast::RightVal, sites: &mut CallSites) {
    match &val.inner {
        ast::RightValInner::ArithExpr(expr) => collect_arith_expr(expr, sites),
        ast::RightValInner::BoolExpr(expr) => collect_bool_expr(expr, sites),
    }
}

fn arith_op(op: &ast::ArithBiOp) -> &'static str {
    match op {
        ast::ArithBiOp::Add => "+",
        ast::ArithBiOp::Sub => "-",
        ast::ArithBiOp::Mul => "*",
        ast::ArithBiOp::Div => "/",
    }
}

fn bool_op(op: &ast::BoolBiOp) -> &'static str {
    match op {
        ast::BoolBiOp::And => "&&",
        ast::BoolBiOp::Or => "||",
    }
}

fn cmp_op(op: &ast::ComOp) -> &'static str {
    match op {
        ast::ComOp::Eq => "==",
        ast::ComOp::Ne => "!=",
        ast::ComOp::Gt => ">",
        ast::ComOp::Ge => ">=",
        ast::ComOp::Lt => "<",
        ast::ComOp::Le => "<=",
    }
}

fn index_expr(expr: &ast::IndexExpr) -> String {
    match &expr.inner {
        ast::IndexExprInner::Num(n) => n.to_string(),
        ast::IndexExprInner::Id(id) => id.clone(),
    }
}

pub(super) fn left_val(val: &ast::LeftVal) -> String {
    match &val.inner {
        ast::LeftValInner::Id(id) => id.clone(),
        ast::LeftValInner::ArrayExpr(expr) => array_expr(expr),
        ast::LeftValInner::MemberExpr(expr) => member_expr(expr),
    }
}

fn array_expr(expr: &ast::ArrayExpr) -> String {
    format!("{}[{}]", left_val(&expr.arr), index_expr(&expr.idx))
}

fn member_expr(expr: &ast::MemberExpr) -> String {
    format!("{}.{}", left_val(&expr.struct_id), expr.member_id)
}

fn fn_call(call: &ast::FnCall) -> String {
    let args = call
        .vals
        .iter()
        .map(right_val)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({args})", call.qualified_name())
}

fn expr_unit(unit: &ast::ExprUnit) -> String {
    match &unit.inner {
        ast::ExprUnitInner::Num(n) => n.to_string(),
        ast::ExprUnitInner::Id(id) => id.clone(),
        ast::ExprUnitInner::ArithExpr(expr) => format!("({})", arith_expr(expr)),
        ast::ExprUnitInner::FnCall(call) => fn_call(call),
        ast::ExprUnitInner::TraceCall(call) => format!("trace {}", fn_call(call)),
        ast::ExprUnitInner::ArrayExpr(expr) => array_expr(expr),
        ast::ExprUnitInner::MemberExpr(expr) => member_expr(expr),
        ast::ExprUnitInner::Reference(id) => format!("&{id}"),
    }
}

fn arith_expr(expr: &ast::ArithExpr) -> String {
    match &expr.inner {
        ast::ArithExprInner::ArithBiOpExpr(expr) => format!(
            "{} {} {}",
            arith_expr(&expr.left),
            arith_op(&expr.op),
            arith_expr(&expr.right)
        ),
        ast::ArithExprInner::ExprUnit(unit) => expr_unit(unit),
    }
}

fn com_expr(expr: &ast::ComExpr) -> String {
    format!(
        "{} {} {}",
        expr_unit(&expr.left),
        cmp_op(&expr.op),
        expr_unit(&expr.right)
    )
}

pub(super) fn bool_unit(unit: &ast::BoolUnit) -> String {
    match &unit.inner {
        ast::BoolUnitInner::ComExpr(expr) => com_expr(expr),
        ast::BoolUnitInner::BoolExpr(expr) => bool_expr(expr),
        ast::BoolUnitInner::BoolUOpExpr(expr) => format!("!{}", bool_unit(&expr.cond)),
    }
}

fn bool_expr(expr: &ast::BoolExpr) -> String {
    match &expr.inner {
        ast::BoolExprInner::BoolBiOpExpr(expr) => format!(
            "{} {} {}",
            bool_expr(&expr.left),
            bool_op(&expr.op),
            bool_expr(&expr.right)
        ),
        ast::BoolExprInner::BoolUnit(unit) => bool_unit(unit),
    }
}

pub(super) fn right_val(val: &ast::RightVal) -> String {
    match &val.inner {
        ast::RightValInner::ArithExpr(expr) => arith_expr(expr),
        ast::RightValInner::BoolExpr(expr) => bool_expr(expr),
    }
}

impl FunctionGenerator<'_> {
    fn emit_trace_runtime(&mut self, name: &str, args: Vec<Operand>) {
        self.emit_call(name.to_string(), None, args);
    }

    fn emit_trace_event(&mut self, indent: usize) {
        self.emit_trace_runtime("__teac_trace_event", vec![Operand::from(indent as i32)]);
    }

    fn emit_trace_text(&mut self, text: &str) {
        for byte in text.bytes() {
            self.emit_trace_runtime("__teac_trace_putch", vec![Operand::from(i32::from(byte))]);
        }
    }

    fn emit_trace_value(&mut self, value: Operand) {
        match value.dtype() {
            Dtype::I1 => self.emit_trace_runtime("__teac_trace_putbool", vec![value]),
            Dtype::I32 => self.emit_trace_runtime("__teac_trace_putint", vec![value]),
            dtype => unreachable!("trace value has unsupported dtype {dtype}"),
        }
    }

    pub(super) fn emit_trace_call_line(
        &mut self,
        indent: usize,
        name: &str,
        args: &[(String, Operand)],
    ) {
        self.emit_trace_event(indent);
        self.emit_trace_text("call ");
        self.emit_trace_text(name);
        self.emit_trace_text("(");
        for (i, (arg_name, value)) in args.iter().enumerate() {
            if i > 0 {
                self.emit_trace_text(", ");
            }
            self.emit_trace_text(arg_name);
            self.emit_trace_text(" = ");
            self.emit_trace_value(value.clone());
        }
        self.emit_trace_text(")\n");
    }

    pub(super) fn emit_trace_function_enter(&mut self) {
        self.emit_trace_runtime("__teac_trace_enter", vec![]);
    }

    pub(super) fn emit_trace_begin_line(&mut self, name: &str, args: &[Operand]) {
        self.emit_trace_runtime("__teac_trace_begin", vec![]);
        self.emit_trace_event(0);
        self.emit_trace_text("trace ");
        self.emit_trace_text(name);
        self.emit_trace_text("(");
        for (i, value) in args.iter().enumerate() {
            if i > 0 {
                self.emit_trace_text(", ");
            }
            self.emit_trace_value(value.clone());
        }
        self.emit_trace_text(")\n");
    }

    pub(super) fn emit_trace_end_line(&mut self) {
        self.emit_trace_event(0);
        self.emit_trace_text("end trace\n");
        self.emit_trace_runtime("__teac_trace_finish", vec![]);
    }

    pub(super) fn emit_trace_function_return(&mut self, indent: usize, value: Option<Operand>) {
        self.emit_trace_event(indent);
        self.emit_trace_text("return");
        if let Some(value) = value {
            self.emit_trace_text(" ");
            self.emit_trace_value(value);
        }
        self.emit_trace_text("\n");
        self.emit_trace_runtime("__teac_trace_leave", vec![]);
    }

    pub(super) fn emit_trace_let_line(&mut self, indent: usize, name: &str, value: Operand) {
        self.emit_trace_event(indent);
        self.emit_trace_text("let ");
        self.emit_trace_text(name);
        self.emit_trace_text(" = ");
        self.emit_trace_value(value);
        self.emit_trace_text("\n");
    }

    pub(super) fn emit_trace_change_line(
        &mut self,
        indent: usize,
        name: &str,
        old: Operand,
        new: Operand,
    ) {
        let print_label = self.alloc_basic_block();
        let after_label = self.alloc_basic_block();
        let cond = Operand::from(self.fresh_local(Dtype::I1));

        self.emit_cmp(CmpPredicate::Ne, old.clone(), new.clone(), cond.clone());
        self.emit_cjump(cond, print_label.clone(), after_label.clone());

        self.emit_label(print_label);
        self.emit_trace_event(indent);
        self.emit_trace_text(name);
        self.emit_trace_text(": ");
        self.emit_trace_value(old);
        self.emit_trace_text(" -> ");
        self.emit_trace_value(new);
        self.emit_trace_text("\n");
        self.emit_jump(after_label.clone());

        self.emit_label(after_label);
    }

    pub(super) fn emit_trace_condition_line(
        &mut self,
        indent: usize,
        kind: &str,
        cond: &str,
        value: Operand,
    ) {
        self.emit_trace_event(indent);
        self.emit_trace_text(kind);
        self.emit_trace_text(" ");
        self.emit_trace_text(cond);
        self.emit_trace_text(" -> ");
        self.emit_trace_runtime("__teac_trace_putbool", vec![value]);
        self.emit_trace_text("\n");
    }

    pub(super) fn emit_trace_else_line(&mut self, indent: usize) {
        self.emit_trace_event(indent);
        self.emit_trace_text("else\n");
    }

    pub(super) fn emit_trace_loop_slot(&mut self) -> Local {
        let slot = self.fresh_local(Dtype::ptr_to(Dtype::I32));
        self.emit_alloca(Operand::from(&slot));
        self.emit_store(Operand::from(0), Operand::from(&slot));
        slot
    }

    pub(super) fn bump_trace_loop_count(&mut self, slot: &Local) -> Operand {
        let cur = Operand::from(self.fresh_local(Dtype::I32));
        let next = Operand::from(self.fresh_local(Dtype::I32));
        self.emit_load(cur.clone(), Operand::from(slot));
        self.emit_biop(ArithBinOp::Add, cur, Operand::from(1), next.clone());
        self.emit_store(next.clone(), Operand::from(slot));
        next
    }

    pub(super) fn bump_trace_loop(&mut self, slot: &Local, indent: usize) -> Operand {
        let next = self.bump_trace_loop_count(slot);
        self.emit_trace_event(indent);
        self.emit_trace_text("loop #");
        self.emit_trace_value(next.clone());
        self.emit_trace_text("\n");
        next
    }
}
