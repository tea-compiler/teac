//! Forward-flow type inference pass.
//!
//! This module resolves the types of all local variables in a function body
//! **before** IR generation.  It walks the AST statements top-to-bottom,
//! maintaining a type environment (`TypeEnv`) that maps each variable name to
//! a resolution state — either `Resolved(Dtype)` or `Pending`.
//!
//! ## Name resolution
//!
//! Inside a function body, three kinds of names are visible:
//!
//! 1. **Function parameters** — seeded into the environment as `Resolved`
//!    at entry from `fn_decl.param_decl`.
//! 2. **Local variables** — tracked in the environment with forward-flow
//!    state (`Resolved`/`Pending`), populated as declarations are walked.
//! 3. **Global variables** — looked up through the module's global list
//!    when a name is not found in the local environment.
//!
//! Any name that resolves to none of the above is a `VariableNotDefined`
//! error.  This mirrors the lookup chain used by
//! [`crate::ir::function::FunctionGenerator::lookup_variable`] at IR
//! generation time, keeping both passes' views of the name environment
//! in sync.
//!
//! ## Inference rules
//!
//! | #  | Form | Effect |
//! |----|------|--------|
//! | R1 | `let x: T;`                           | x → Resolved(T) |
//! | R2 | `let x: T = e;`                       | check typeOf(e) = T; x → Resolved(T) |
//! | R3 | `let x = e;`                          | t = typeOf(e), must be concrete; x → Resolved(t) |
//! | R4 | `let x;`                              | x → Pending |
//! | R5 | `x = e;` where x is Pending           | resolve x to typeOf(e) |
//! | R6 | `x = e;` where x is Resolved(T)       | check typeOf(e) = T |
//! | R7 | `if … else …`                         | process each arm independently, then merge |
//! | R8 | `while …`                             | process the body once, then merge back |
//!
//! ## Boundaries
//!
//! **Supported:** forward inference from initializers and first assignments,
//! including across if/else branches and loop bodies.
//!
//! **Not supported (compile error):** using a variable whose type has not yet
//! been determined, chains of unresolved variables, backward inference from
//! usage context, and function signature inference.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::ast;
use crate::ir::module::Registry;
use crate::ir::types::Dtype;
use crate::ir::value::GlobalDef;
use crate::ir::Error;
use std::rc::Rc;

/// Resolution state for a local variable during type inference.
#[derive(Clone, Debug)]
enum VarState {
    /// The variable's type has been determined.
    Resolved(Dtype),
    /// The variable was declared without a type annotation and has not yet
    /// been assigned to — its type is still unknown.
    Pending,
}

/// Type environment mapping variable names to their resolution state.
type TypeEnv = HashMap<String, VarState>;

/// Stateful walker that performs forward-flow type inference on a single
/// function body.
///
/// The inference context holds the three kinds of name-environment data
/// needed to resolve any identifier appearing in the function body:
/// the type registry for struct and function signatures, the module's
/// global variable list, and the mutable local environment.
struct TypeInference<'a> {
    registry: &'a Registry,
    globals: &'a IndexMap<Rc<str>, GlobalDef>,
    env: TypeEnv,
}

impl TypeInference<'_> {
    /// Creates a child context that shares the module-level references with
    /// `self` but carries an independent local environment.  Used when
    /// processing branches (if/else arms, loop bodies) that need to type-check
    /// in isolation before being merged back.
    fn fork(&self, env: TypeEnv) -> Self {
        Self {
            registry: self.registry,
            globals: self.globals,
            env,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Resolve the types of all local variables in `fn_def`.
///
/// Returns a map from variable name to its concrete [`Dtype`].  Any variable
/// whose type cannot be determined causes an error.
pub fn infer_function(
    registry: &Registry,
    globals: &IndexMap<Rc<str>, GlobalDef>,
    fn_def: &ast::FnDef,
) -> Result<HashMap<String, Dtype>, Error> {
    let mut ctx = TypeInference {
        registry,
        globals,
        env: TypeEnv::new(),
    };

    // Seed the environment with the function's parameters so that references
    // to parameter names resolve to their declared types (not to a silent
    // I32 fallback).  Parameters always have a concrete, explicitly declared
    // type, so they go straight into `Resolved`.
    if let Some(params) = &fn_def.fn_decl.param_decl {
        for decl in &params.decls {
            let dtype = Dtype::try_from(decl)?;
            ctx.env
                .insert(decl.identifier.clone(), VarState::Resolved(dtype));
        }
    }

    for stmt in &fn_def.stmts {
        ctx.process_stmt(stmt)?;
    }

    let mut resolved = HashMap::new();
    for (name, state) in &ctx.env {
        match state {
            VarState::Resolved(dtype) => {
                resolved.insert(name.clone(), dtype.clone());
            }
            VarState::Pending => {
                return Err(Error::TypeNotDetermined {
                    symbol: name.clone(),
                });
            }
        }
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Statement processing
// ---------------------------------------------------------------------------

impl TypeInference<'_> {
    fn process_stmt(&mut self, stmt: &ast::CodeBlockStmt) -> Result<(), Error> {
        match &stmt.inner {
            ast::CodeBlockStmtInner::VarDecl(s) => match &s.inner {
                ast::VarDeclStmtInner::Decl(d) => {
                    self.process_var_decl(d);
                    Ok(())
                }
                ast::VarDeclStmtInner::Def(d) => self.process_var_def(d),
            },
            ast::CodeBlockStmtInner::Assignment(s) => self.process_assignment(s),
            ast::CodeBlockStmtInner::If(s) => self.process_if(s),
            ast::CodeBlockStmtInner::While(s) => self.process_while(s),
            ast::CodeBlockStmtInner::Call(s) => self.check_call_args(&s.fn_call),
            ast::CodeBlockStmtInner::Return(s) => self.process_return(s),
            ast::CodeBlockStmtInner::Continue(_)
            | ast::CodeBlockStmtInner::Break(_)
            | ast::CodeBlockStmtInner::Null(_) => Ok(()),
        }
    }

    fn process_stmts(&mut self, stmts: &[ast::CodeBlockStmt]) -> Result<(), Error> {
        for stmt in stmts {
            self.process_stmt(stmt)?;
        }
        Ok(())
    }

    // -- Variable declaration (no initializer) --------------------------------

    /// R1 (`let x: T;`) and R4 (`let x;`).
    fn process_var_decl(&mut self, decl: &ast::VarDecl) {
        let id = &decl.identifier;
        let state = match (&decl.type_specifier, &decl.inner) {
            // Typed scalar: let x: T;
            (Some(ts), ast::VarDeclInner::Scalar) => VarState::Resolved(Dtype::from(ts)),
            // Typed array: let x: [T; N];
            (Some(ts), ast::VarDeclInner::Array(arr)) => {
                VarState::Resolved(Dtype::array_of(Dtype::from(ts), arr.len))
            }
            // Untyped array (defaults to i32 elements): let x: [; N] — grammatically
            // rare but handled for completeness.
            (None, ast::VarDeclInner::Array(arr)) => {
                VarState::Resolved(Dtype::array_of(Dtype::I32, arr.len))
            }
            // Untyped scalar: let x;
            (None, ast::VarDeclInner::Scalar) => VarState::Pending,
        };
        self.env.insert(id.clone(), state);
    }

    // -- Variable definition (with initializer) -------------------------------

    /// R2 (`let x: T = e;`) and R3 (`let x = e;`).
    fn process_var_def(&mut self, def: &ast::VarDef) -> Result<(), Error> {
        let id = &def.identifier;
        let explicit_dtype = def.type_specifier.as_ref().map(Dtype::from);

        match &def.inner {
            ast::VarDefInner::Scalar(scalar) => {
                let rhs_type = self.type_of_right_val(&scalar.val)?;
                let resolved = match &explicit_dtype {
                    Some(t) => {
                        Self::check_compatible(id, t, &rhs_type)?;
                        t.clone()
                    }
                    None => rhs_type,
                };
                self.env.insert(id.clone(), VarState::Resolved(resolved));
            }
            ast::VarDefInner::Array(arr) => {
                let elem_type = match &explicit_dtype {
                    Some(t) => t.clone(),
                    None => Dtype::I32,
                };
                self.check_array_initializer(&arr.initializer)?;
                self.env.insert(
                    id.clone(),
                    VarState::Resolved(Dtype::array_of(elem_type, arr.len)),
                );
            }
        }
        Ok(())
    }

    fn check_array_initializer(&self, init: &ast::ArrayInitializer) -> Result<(), Error> {
        match init {
            ast::ArrayInitializer::ExplicitList(vals) => {
                for v in vals {
                    self.type_of_right_val(v)?;
                }
            }
            ast::ArrayInitializer::Fill { val, .. } => {
                self.type_of_right_val(val)?;
            }
        }
        Ok(())
    }

    // -- Assignment -----------------------------------------------------------

    /// R5 / R6: `x = e;` — resolve a Pending local to typeOf(e), or check
    /// type compatibility against an already-Resolved local.
    fn process_assignment(&mut self, stmt: &ast::AssignmentStmt) -> Result<(), Error> {
        let rhs_type = self.type_of_right_val(&stmt.right_val)?;

        match &stmt.left_val.inner {
            ast::LeftValInner::Id(id) => {
                let state = self.env.get(id).cloned();
                match state {
                    Some(VarState::Pending) => {
                        self.env.insert(id.clone(), VarState::Resolved(rhs_type));
                    }
                    Some(VarState::Resolved(t)) => {
                        Self::check_compatible(id, &t, &rhs_type)?;
                    }
                    None => {
                        // Variable not in local env — it may be a global.
                        // We don't track globals in this pass; IR gen will
                        // catch undefined references.
                    }
                }
            }
            ast::LeftValInner::ArrayExpr(expr) => {
                self.type_of_left_val_array(expr)?;
            }
            ast::LeftValInner::MemberExpr(expr) => {
                self.type_of_member_expr(expr)?;
            }
        }
        Ok(())
    }

    // -- Branching (if/else) --------------------------------------------------

    /// R7: if/else merging.
    fn process_if(&mut self, stmt: &ast::IfStmt) -> Result<(), Error> {
        self.check_bool_unit(&stmt.bool_unit)?;

        let mut then_ctx = self.fork(self.env.clone());
        then_ctx.process_stmts(&stmt.if_stmts)?;
        let then_env = then_ctx.env;

        let else_env = if let Some(else_stmts) = &stmt.else_stmts {
            let mut else_ctx = self.fork(self.env.clone());
            else_ctx.process_stmts(else_stmts)?;
            else_ctx.env
        } else {
            self.env.clone()
        };

        self.merge_envs(&then_env, &else_env)?;
        Ok(())
    }

    // -- Loops ----------------------------------------------------------------

    /// R8: while merging.
    fn process_while(&mut self, stmt: &ast::WhileStmt) -> Result<(), Error> {
        self.check_bool_unit(&stmt.bool_unit)?;

        let mut body_ctx = self.fork(self.env.clone());
        body_ctx.process_stmts(&stmt.stmts)?;
        let body_env = body_ctx.env;

        self.merge_env_single(&body_env)?;
        Ok(())
    }

    // -- Return ---------------------------------------------------------------

    fn process_return(&mut self, stmt: &ast::ReturnStmt) -> Result<(), Error> {
        if let Some(val) = &stmt.val {
            self.type_of_right_val(val)?;
        }
        Ok(())
    }

    // -- Environment merging --------------------------------------------------

    /// Merge two branch environments back into `self.env`.
    ///
    /// For each variable already in the pre-branch environment:
    /// - Pending + Pending  → Pending
    /// - Pending + Resolved → Resolved (type is learned from one branch)
    /// - Resolved + Pending → Resolved
    /// - Resolved(T) + Resolved(T) → Resolved(T)
    /// - Resolved(T1) + Resolved(T2), T1 ≠ T2 → Error
    fn merge_envs(&mut self, env_a: &TypeEnv, env_b: &TypeEnv) -> Result<(), Error> {
        let base = self.env.clone();
        for (name, base_state) in &base {
            let state_a = env_a.get(name).unwrap_or(base_state);
            let state_b = env_b.get(name).unwrap_or(base_state);
            let merged = Self::merge_states(name, state_a, state_b)?;
            self.env.insert(name.clone(), merged);
        }
        Ok(())
    }

    /// Merge the single branch environment produced by a loop body back into
    /// `self.env`.  Equivalent to `merge_envs(branch_env, self.env)` — the
    /// loop body is treated as an "optional" branch, since the body may run
    /// zero times and types learned inside must be reconciled with the
    /// pre-loop state.
    fn merge_env_single(&mut self, branch_env: &TypeEnv) -> Result<(), Error> {
        let base = self.env.clone();
        for (name, base_state) in &base {
            let branch_state = branch_env.get(name).unwrap_or(base_state);
            let merged = Self::merge_states(name, base_state, branch_state)?;
            self.env.insert(name.clone(), merged);
        }
        Ok(())
    }

    fn merge_states(name: &str, a: &VarState, b: &VarState) -> Result<VarState, Error> {
        match (a, b) {
            (VarState::Pending, VarState::Pending) => Ok(VarState::Pending),
            (VarState::Resolved(t), VarState::Pending)
            | (VarState::Pending, VarState::Resolved(t)) => Ok(VarState::Resolved(t.clone())),
            (VarState::Resolved(t1), VarState::Resolved(t2)) => {
                if t1 == t2 {
                    Ok(VarState::Resolved(t1.clone()))
                } else {
                    Err(Error::TypeMismatch {
                        symbol: name.to_string(),
                        expected: t1.clone(),
                        actual: t2.clone(),
                    })
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expression typing
// ---------------------------------------------------------------------------

impl TypeInference<'_> {
    /// Compute the type of a right-hand-side value.
    fn type_of_right_val(&self, val: &ast::RightVal) -> Result<Dtype, Error> {
        match &val.inner {
            ast::RightValInner::ArithExpr(expr) => self.type_of_arith_expr(expr),
            ast::RightValInner::BoolExpr(expr) => {
                self.check_bool_expr(expr)?;
                Ok(Dtype::I32)
            }
        }
    }

    /// Compute the type of an arithmetic expression.
    fn type_of_arith_expr(&self, expr: &ast::ArithExpr) -> Result<Dtype, Error> {
        match &expr.inner {
            ast::ArithExprInner::ArithBiOpExpr(biop) => {
                self.type_of_arith_expr(&biop.left)?;
                self.type_of_arith_expr(&biop.right)?;
                Ok(Dtype::I32)
            }
            ast::ArithExprInner::ExprUnit(unit) => self.type_of_expr_unit(unit),
        }
    }

    /// Compute the type of a leaf expression unit.
    fn type_of_expr_unit(&self, unit: &ast::ExprUnit) -> Result<Dtype, Error> {
        match &unit.inner {
            ast::ExprUnitInner::Num(_) => Ok(Dtype::I32),
            ast::ExprUnitInner::Id(id) => self.resolve_variable(id),
            ast::ExprUnitInner::ArithExpr(expr) => self.type_of_arith_expr(expr),
            ast::ExprUnitInner::FnCall(call) => self.type_of_fn_call(call),
            ast::ExprUnitInner::ArrayExpr(expr) => self.type_of_array_expr(expr),
            ast::ExprUnitInner::MemberExpr(expr) => self.type_of_member_expr(expr),
            ast::ExprUnitInner::Reference(id) => self.type_of_reference(id),
        }
    }

    /// Resolves a name through the full lookup chain: local env → globals → error.
    ///
    /// This is the single source of truth for "what is the storage type of the
    /// entity named `id`?" inside a function body.  Mirrors the lookup order
    /// used by [`crate::ir::function::FunctionGenerator::lookup_variable`] so
    /// that type inference and IR generation agree on every name.
    ///
    /// * A local entry in `Resolved(T)` state yields `T`.
    /// * A local entry in `Pending` state is a "used before type determined"
    ///   error.
    /// * A name absent from the local env but present in the module's global
    ///   list yields the global's declared type.
    /// * Otherwise the name is undefined.
    fn lookup_dtype(&self, id: &str) -> Result<Dtype, Error> {
        match self.env.get(id) {
            Some(VarState::Resolved(dtype)) => Ok(dtype.clone()),
            Some(VarState::Pending) => Err(Error::TypeNotDetermined {
                symbol: id.to_string(),
            }),
            None => self
                .globals
                .get(id)
                .map(|gv| gv.dtype.clone())
                .ok_or_else(|| Error::VariableNotDefined {
                    symbol: id.to_string(),
                }),
        }
    }

    /// The type of a variable **used as an rvalue** (bare identifier in an
    /// expression).  Arrays decay to their element type, matching the existing
    /// inference semantics for `let x = arr;`-style expressions.
    fn resolve_variable(&self, id: &str) -> Result<Dtype, Error> {
        let dtype = self.lookup_dtype(id)?;
        Ok(match dtype {
            Dtype::Array { element, .. } => element.as_ref().clone(),
            other => other,
        })
    }

    /// Type of a function call expression.
    fn type_of_fn_call(&self, call: &ast::FnCall) -> Result<Dtype, Error> {
        self.check_call_args(call)?;
        let name = call.qualified_name();
        match self.registry.function_types.get(&name) {
            Some(ft) => Ok(ft.return_dtype.clone()),
            None => Err(Error::FunctionNotDefined { symbol: name }),
        }
    }

    fn check_call_args(&self, call: &ast::FnCall) -> Result<(), Error> {
        for arg in &call.vals {
            self.type_of_right_val(arg)?;
        }
        Ok(())
    }

    /// Type of an array element access (yields the element type).
    fn type_of_array_expr(&self, expr: &ast::ArrayExpr) -> Result<Dtype, Error> {
        let arr_type = self.type_of_left_val(&expr.arr)?;
        Ok(Self::element_type_of_indexing(&arr_type))
    }

    /// Given the type of `arr`, return the element type yielded by `arr[i]`.
    ///
    /// Handles both forms that can appear as an indexable base:
    /// * `Array<T, N>` — a local or global array variable.
    /// * `Pointer<Array<T, ?>>` — a function parameter declared as `&[T]`,
    ///   whose storage type carries an extra pointer layer.
    ///
    /// Mirrors the pointer-unwrapping logic in
    /// [`crate::ir::function::FunctionGenerator::handle_array_expr`] so that
    /// the two passes agree on the element type produced by an index.
    ///
    /// Any other input is returned unchanged; the IR generation pass is the
    /// authoritative checker for invalid indexing (`InvalidArrayExpression`).
    fn element_type_of_indexing(dtype: &Dtype) -> Dtype {
        match dtype {
            Dtype::Array { element, .. } => element.as_ref().clone(),
            Dtype::Pointer { pointee } => match pointee.as_ref() {
                Dtype::Array { element, .. } => element.as_ref().clone(),
                _ => dtype.clone(),
            },
            _ => dtype.clone(),
        }
    }

    /// Type of a struct member access.
    ///
    /// Uses [`Dtype::struct_type_name`] so that member access transparently
    /// sees through `Pointer<..>` and `Array<..>` wrappers, matching the
    /// behaviour of [`FunctionGenerator::handle_member_expr`] at IR
    /// generation time.
    ///
    /// [`FunctionGenerator::handle_member_expr`]: crate::ir::function::FunctionGenerator
    fn type_of_member_expr(&self, expr: &ast::MemberExpr) -> Result<Dtype, Error> {
        let struct_type = self.type_of_left_val(&expr.struct_id)?;
        let type_name = struct_type
            .struct_type_name()
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })?;
        let st = self
            .registry
            .struct_types
            .get(type_name)
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })?;
        st.elements
            .iter()
            .find(|(name, _)| name == &expr.member_id)
            .map(|(_, member)| member.dtype.clone())
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })
    }

    /// Type of a reference expression `&x`.
    ///
    /// The reference operand may be either a bare array (local/global array
    /// variable) or a pointer to an array (a function parameter declared as
    /// `&[T]`, whose storage type is `Pointer<Array<T, None>>`).  In both
    /// cases the result is `Pointer<Array<element, None>>`, matching
    /// [`FunctionGenerator::handle_reference_expr`] at IR generation time.
    ///
    /// [`FunctionGenerator::handle_reference_expr`]: crate::ir::function::FunctionGenerator
    fn type_of_reference(&self, id: &str) -> Result<Dtype, Error> {
        let var_type = self.lookup_dtype(id)?;
        let element_type = match var_type {
            Dtype::Array { element, .. } => element.as_ref().clone(),
            Dtype::Pointer { pointee } => match *pointee {
                Dtype::Array { element, .. } => element.as_ref().clone(),
                _ => {
                    return Err(Error::InvalidReference {
                        symbol: id.to_string(),
                    });
                }
            },
            _ => {
                return Err(Error::InvalidReference {
                    symbol: id.to_string(),
                });
            }
        };
        Ok(Dtype::ptr_to(Dtype::Array {
            element: Box::new(element_type),
            length: None,
        }))
    }

    /// Resolve the type of a left-value (for type-checking, not IR gen).
    fn type_of_left_val(&self, val: &ast::LeftVal) -> Result<Dtype, Error> {
        match &val.inner {
            ast::LeftValInner::Id(id) => self.lookup_dtype(id),
            ast::LeftValInner::ArrayExpr(expr) => self.type_of_left_val_array(expr),
            ast::LeftValInner::MemberExpr(expr) => self.type_of_member_expr(expr),
        }
    }

    fn type_of_left_val_array(&self, expr: &ast::ArrayExpr) -> Result<Dtype, Error> {
        let arr_type = self.type_of_left_val(&expr.arr)?;
        Ok(Self::element_type_of_indexing(&arr_type))
    }

    // -- Boolean expression checking (just validates sub-expressions) ----------

    fn check_bool_expr(&self, expr: &ast::BoolExpr) -> Result<(), Error> {
        match &expr.inner {
            ast::BoolExprInner::BoolBiOpExpr(biop) => {
                self.check_bool_expr(&biop.left)?;
                self.check_bool_expr(&biop.right)
            }
            ast::BoolExprInner::BoolUnit(unit) => self.check_bool_unit(unit),
        }
    }

    fn check_bool_unit(&self, unit: &ast::BoolUnit) -> Result<(), Error> {
        match &unit.inner {
            ast::BoolUnitInner::ComExpr(expr) => {
                self.type_of_expr_unit(&expr.left)?;
                self.type_of_expr_unit(&expr.right)?;
                Ok(())
            }
            ast::BoolUnitInner::BoolExpr(expr) => self.check_bool_expr(expr),
            ast::BoolUnitInner::BoolUOpExpr(expr) => self.check_bool_unit(&expr.cond),
        }
    }

    // -- Type compatibility check ---------------------------------------------

    fn check_compatible(symbol: &str, expected: &Dtype, actual: &Dtype) -> Result<(), Error> {
        if expected == actual {
            return Ok(());
        }
        Err(Error::TypeMismatch {
            symbol: symbol.to_string(),
            expected: expected.clone(),
            actual: actual.clone(),
        })
    }
}
