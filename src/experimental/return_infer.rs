//! Pluggable return-type inference pass (asmt-2).
//!
//! This module is a **self-contained, feature-gated extension** of the
//! compiler.  It runs between Pass 2 (signature registration) and Pass 3
//! (per-function type inference + IR generation) and fills in the return
//! type of every function that was declared without an explicit `-> T`
//! clause.  The rest of the pipeline is not touched — the forward-flow
//! `type_infer` pass (inside the otherwise-private `ir::gen` module tree)
//! and the [`FunctionGenerator`] see only concrete [`Dtype`]s after we
//! are done.
//!
//! [`FunctionGenerator`]: crate::ir::function::FunctionGenerator
//!
//! # How it fits in
//!
//! When the `return-type-inference` Cargo feature is enabled:
//!
//! ```text
//! Pass 1  -> Pass 2  -> [Pass 2.5: this module]  -> Pass 3
//!                         |
//!                         | mutates Registry.function_types[*].return_dtype
//!                         v
//!                       Registry has every return type concrete.
//! ```
//!
//! When the feature is disabled, this module is not compiled at all;
//! omitted return types silently remain `void` (the pre-asmt-2 behaviour).
//!
//! # Algorithm
//!
//! 1. **Seed**.  For every `FnDef` whose declaration has `return_dtype =
//!    None`, allocate a fresh type variable `α_f` in a per-module
//!    [`UnionFind`] and record the mapping in `pending_returns`.
//!
//! 2. **Collect**.  Walk every function body (pending or not).  For each
//!    expression, compute a [`Ty`] that may be concrete or a type
//!    variable.  Emit a [`unify`] call for:
//!
//!    - `let x: T = e;` — unify(T, typeOf(e))
//!    - `let x = e;` — bind x in the local env to typeOf(e)
//!    - `x = e;` — unify(typeOf(x), typeOf(e))
//!    - branch merging — unify across branch environments
//!    - `return e;` inside a pending function — unify(α_f, typeOf(e))
//!    - `return;` inside a pending function — unify(α_f, void)
//!    - every call to a pending callee returns `Ty::Var(α_callee)`
//!
//! 3. **Resolve**.  For every `(name, α_f)` in `pending_returns`, query
//!    the union-find for the concrete type of `α_f` and write it back
//!    to `registry.function_types[name].return_dtype`.  Unresolved
//!    variables and non-`void`/`i32` resolutions are errors.
//!
//! Steps 2 and 3 use a single shared [`UnionFind`]: constraints from one
//! function's body can resolve the return type of another — that is the
//! whole point of doing this globally.

use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::ast;
use crate::common::pass::ModulePass;
use crate::ir::compose_var_def_dtype;
use crate::ir::module::{IrGenerator, Registry};
use crate::ir::types::Dtype;
use crate::ir::value::GlobalDef;
use crate::ir::Error;

// ---------------------------------------------------------------------------
// Plug-in wrapper
// ---------------------------------------------------------------------------

/// [`ModulePass`] wrapper around [`resolve_return_types`].
pub(crate) struct ReturnInferPass;

impl ModulePass for ReturnInferPass {
    fn run(&self, gen: &mut IrGenerator<'_>) -> Result<(), Error> {
        resolve_return_types(&mut gen.registry, &gen.input.elements, &gen.module.global_list)
    }
}

// ---------------------------------------------------------------------------
// Core data types: Ty, TypeId, UnionFind
// ---------------------------------------------------------------------------

/// Unique identifier for a type variable.  A small integer; cheap to copy.
type TypeId = usize;

/// A type *in progress*: either a fully-known [`Dtype`] or an unresolved
/// placeholder waiting for unification to pin it down.
#[derive(Clone, Debug)]
enum Ty {
    /// A fully known type.
    Concrete(Dtype),
    /// A type variable represented by its union-find id.
    Var(TypeId),
}

impl Ty {
    /// Convenience constructor — wraps a `Dtype` as a concrete `Ty`.
    fn concrete(dtype: Dtype) -> Self {
        Self::Concrete(dtype)
    }
}

/// Union-find over type variables.  Each equivalence class's root
/// optionally carries the concrete [`Dtype`] every variable in the class
/// has been committed to.
struct UnionFind {
    parent: Vec<TypeId>,
    rank: Vec<u32>,
    /// Concrete type bound to each *root*, or `None` if still unbound.
    /// Slots for non-root entries are unused.
    concrete: Vec<Option<Dtype>>,
}

impl UnionFind {
    fn new() -> Self {
        Self {
            parent: Vec::new(),
            rank: Vec::new(),
            concrete: Vec::new(),
        }
    }

    /// Allocate a fresh unbound type variable; return its id.
    fn fresh(&mut self) -> TypeId {
        let id = self.parent.len();
        self.parent.push(id);
        self.rank.push(0);
        self.concrete.push(None);
        id
    }

    /// Find the representative of `x`'s equivalence class, with path
    /// compression.
    fn find(&mut self, x: TypeId) -> TypeId {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut cur = x;
        while self.parent[cur] != root {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }

    /// Bind the concrete type `dtype` to `x`'s equivalence class.
    ///
    /// - If the class has no concrete binding yet, install `dtype`.
    /// - If it already has the **same** concrete binding, this is a no-op.
    /// - If it already has a **different** concrete binding, return
    ///   [`Error::TypeMismatch`] tagged with `symbol` so the caller can
    ///   surface a useful diagnostic.
    ///
    /// See `docs/asmt-2.md` §3.2 (核心不变式) and §3.3 (工作示例).
    #[allow(unused_variables)]
    fn bind(&mut self, x: TypeId, dtype: Dtype, symbol: &str) -> Result<(), Error> {
        // TODO(asmt-2 §3.3): Implement `bind`.
        //
        // Steps:
        //   1. Find the root of x (path compression happens inside `find`).
        //   2. Inspect `self.concrete[root]`:
        //        - `None`            → store `Some(dtype)`.
        //        - `Some(existing)`  → if `*existing == dtype`, do nothing;
        //                              otherwise return `Error::TypeMismatch`
        //                              (expected: existing, actual: dtype).
        //
        // The core invariant you are maintaining: **every equivalence class
        // has at most one concrete type**.
        todo!("asmt-2 §3.3: UnionFind::bind")
    }

    /// Merge the equivalence classes containing `a` and `b` (union by rank).
    ///
    /// Merging is the tricky operation, because each side may independently
    /// already carry a concrete binding:
    ///
    /// | lhs concrete | rhs concrete | merged class's binding |
    /// |--------------|--------------|------------------------|
    /// | None         | None         | None                   |
    /// | Some(T)      | None         | Some(T)                |
    /// | None         | Some(T)      | Some(T)                |
    /// | Some(T)      | Some(T)      | Some(T)                |
    /// | Some(Tₐ)     | Some(Tᵦ), Tₐ≠Tᵦ | error (TypeMismatch) |
    ///
    /// See `docs/asmt-2.md` §3.3 (工作示例) — the `union(α₂, α₀)` step is
    /// exactly the "one side concrete, the other empty" row; the `bind`
    /// conflict at the end demonstrates the last row in miniature.
    #[allow(unused_variables)]
    fn union(&mut self, a: TypeId, b: TypeId, symbol: &str) -> Result<(), Error> {
        // TODO(asmt-2 §3.3): Implement `union`.
        //
        // Steps:
        //   1. Find the two roots `ra`, `rb`. If they coincide, return Ok.
        //   2. Combine `self.concrete[ra]` and `self.concrete[rb]` per the
        //      table above; on conflict, return `Error::TypeMismatch`.
        //   3. Hang the lower-rank tree under the higher-rank one
        //      (use `self.rank`). When ranks tie, pick either root and
        //      bump its rank by 1.
        //   4. Write the merged concrete binding into the chosen root's
        //      slot; the other slot is unused from now on.
        todo!("asmt-2 §3.3: UnionFind::union")
    }

    /// Return the concrete type currently bound to `x`'s equivalence class,
    /// or `None` if no constraint has pinned the class to a concrete type
    /// yet.  The resolve phase of Pass 2.5 calls this exactly once per
    /// pending function.
    fn resolve(&mut self, x: TypeId) -> Option<Dtype> {
        // TODO(asmt-2 §3.3): Implement `resolve`.
        //
        // Return `self.concrete[find(x)].clone()`.  Make sure to go through
        // `find` so that path compression is triggered and the answer
        // reflects all unions performed so far.
        todo!("asmt-2 §3.3: UnionFind::resolve")
    }
}

/// Unify two types under the current constraint store.
///
/// This is the **heart** of the whole pass — every constraint the
/// collector emits goes through here.  The three cases mirror the ones
/// in `docs/asmt-2.md` §3.2:
///
/// | a              | b              | action                       |
/// |----------------|----------------|------------------------------|
/// | `Concrete(x)`  | `Concrete(y)`  | require `x == y`, else error |
/// | `Var(v)`       | `Concrete(c)`  | `uf.bind(v, c, symbol)`      |
/// | `Concrete(c)`  | `Var(v)`       | (same — order doesn't matter)|
/// | `Var(x)`       | `Var(y)`       | `uf.union(x, y, symbol)`     |
///
/// `symbol` is the identifier the caller wants blamed in any diagnostic
/// (usually a variable name, a function name, or `self.fn_name`).
#[allow(unused_variables)]
fn unify(uf: &mut UnionFind, a: &Ty, b: &Ty, symbol: &str) -> Result<(), Error> {
    // TODO(asmt-2 §3.4): Implement the three-way match on `(a, b)`.
    //
    // For the Concrete/Concrete arm, format the error the same way the
    // UnionFind operations do (`Error::TypeMismatch { symbol, expected,
    // actual }`) so the diagnostic stays consistent regardless of which
    // branch fails.
    todo!("asmt-2 §3.4: unify — three cases")
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run Pass 2.5: resolve every omitted function return type in `elements`
/// and write the results back into `registry`.
///
/// This is the single public API of the module.  After it returns `Ok`,
/// every entry in `registry.function_types` has a concrete, non-placeholder
/// `return_dtype`.
///
/// # Errors
///
/// - [`Error::TypeMismatch`] — two return sites (directly or transitively)
///   disagree on a function's return type.
/// - [`Error::TypeNotDetermined`] — a function has omitted its return
///   type but nothing in the program pins it to a concrete type (e.g. a
///   function whose body only calls itself).
/// - [`Error::UnsupportedReturnType`] — inference produced a type other
///   than `void` or `i32`; the backend does not support those yet.
#[allow(unused_variables)]
pub(crate) fn resolve_return_types(
    registry: &mut Registry,
    elements: &[ast::ProgramElement],
    globals: &IndexMap<Rc<str>, GlobalDef>,
) -> Result<(), Error> {
    let mut uf = UnionFind::new();

    // -----------------------------------------------------------------------
    // Phase 1 — Seed (already implemented for you).
    //
    // For every FnDef whose declaration has no explicit return type, hand
    // it a fresh α from the UnionFind and remember the mapping keyed by
    // the function name.  This phase is pure book-keeping — there is no
    // algorithmic insight here, so the skeleton fills it in to keep
    // pre-asmt-2 regression tests (every function has `-> T`) green
    // without requiring any student code.
    // -----------------------------------------------------------------------
    let mut pending_returns: HashMap<String, TypeId> = HashMap::new();
    for elem in elements {
        if let ast::ProgramElementInner::FnDef(fn_def) = &elem.inner {
            if fn_def.fn_decl.return_dtype.is_none() {
                let id = uf.fresh();
                pending_returns.insert(fn_def.fn_decl.identifier.clone(), id);
            }
        }
    }

    // Early exit: if no function in the program omitted its return type,
    // there is nothing to infer.  This short-circuit is what makes the
    // pass a no-op for every non-asmt-2 test program.
    if pending_returns.is_empty() {
        return Ok(());
    }

    // TODO(asmt-2 §4.3): Phase 2 — Collect.
    //
    //   Walk every FnDef body (both pending *and* non-pending — a
    //   non-pending body may still call pending callees, and those
    //   call sites contribute constraints we need).  Delegate each
    //   body to `collect_constraints`, threading the same `uf` and
    //   `pending_returns` through every call so constraints from one
    //   body can pin the return type of another.
    //
    // TODO(asmt-2 §4.3): Phase 3 — Resolve.
    //
    //   For each `(name, α_f)` in `pending_returns`:
    //     1. `uf.resolve(α_f)` → `Option<Dtype>`.
    //        - `None` means no constraint ever pinned this class.
    //          Fall back to `Dtype::Void`, matching the pre-asmt-2
    //          semantics for a body with no `return`.
    //     2. Reject anything other than `Void` / `I32` with
    //        `Error::UnsupportedReturnType` (the aarch64 backend does
    //        not lower other types).
    //     3. Overwrite `registry.function_types[name].return_dtype`
    //        with the resolved type.  Use `Error::FunctionNotDefined`
    //        if the name somehow isn't in the registry (shouldn't
    //        happen after Pass 2).
    //
    //   Conflicts (e.g. `type_infer_5`: both `return;` and `return t;`
    //   in one body) surface inside `unify` during Phase 2; this
    //   phase does not need any extra conflict checks.
    todo!("asmt-2 §4.3: collect + resolve")
}

// ---------------------------------------------------------------------------
// Constraint collection (per function body)
// ---------------------------------------------------------------------------

/// Per-function walker that emits unification constraints into the
/// shared [`UnionFind`].  Operates in the [`Ty`] domain so that
/// unresolved return types can flow through the walk.
struct Collector<'a> {
    registry: &'a Registry,
    globals: &'a IndexMap<Rc<str>, GlobalDef>,
    pending: &'a HashMap<String, TypeId>,
    uf: &'a mut UnionFind,
    /// Function name — only used as a `symbol` in diagnostics.
    fn_name: &'a str,
    /// `Some(α_f)` for a function with an omitted return type; `None` when
    /// the return type was given explicitly.
    return_var: Option<TypeId>,
    /// Local variable environment.  Every entry is a `Ty` because locals
    /// initialised from pending-function calls are unresolved until Pass
    /// 2.5 finishes solving.
    env: HashMap<String, Ty>,
}

/// Drive constraint collection for one function body.  All state is
/// thrown away at the end; only the mutations to `uf` and `pending`
/// propagate to the next call.
fn collect_constraints(
    registry: &Registry,
    globals: &IndexMap<Rc<str>, GlobalDef>,
    pending: &HashMap<String, TypeId>,
    uf: &mut UnionFind,
    fn_def: &ast::FnDef,
) -> Result<(), Error> {
    let fn_name = fn_def.fn_decl.identifier.as_str();
    let return_var = pending.get(fn_name).copied();

    let mut env: HashMap<String, Ty> = HashMap::new();
    if let Some(params) = &fn_def.fn_decl.param_decl {
        for decl in &params.decls {
            let dtype = Dtype::try_from(decl)?;
            env.insert(decl.identifier.clone(), Ty::concrete(dtype));
        }
    }

    let mut ctx = Collector {
        registry,
        globals,
        pending,
        uf,
        fn_name,
        return_var,
        env,
    };

    for stmt in &fn_def.stmts {
        ctx.process_stmt(stmt)?;
    }
    Ok(())
}

impl Collector<'_> {
    /// Fork a child context for a branch (if/else arm, loop body) that
    /// needs to type-check with an independent local environment before
    /// being merged back.  The shared `uf` and `pending` stay shared —
    /// constraints emitted inside a branch are global.
    fn fork(&mut self, env: HashMap<String, Ty>) -> Collector<'_> {
        Collector {
            registry: self.registry,
            globals: self.globals,
            pending: self.pending,
            uf: self.uf,
            fn_name: self.fn_name,
            return_var: self.return_var,
            env,
        }
    }

    // -----------------------------------------------------------------------
    // Statement dispatch
    // -----------------------------------------------------------------------

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
            ast::CodeBlockStmtInner::Call(s) => {
                self.type_of_fn_call(&s.fn_call)?;
                Ok(())
            }
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

    // -----------------------------------------------------------------------
    // Variable declaration (no initialiser)
    // -----------------------------------------------------------------------

    fn process_var_decl(&mut self, decl: &ast::VarDecl) {
        // Untyped scalars are left out of the env: inferring their type
        // is Pass 3's job, and their first assignment will publish a
        // concrete binding (see `process_assignment`).
        let ty = match (&decl.type_specifier, &decl.inner) {
            (Some(ts), ast::VarDeclInner::Scalar) => Some(Ty::concrete(Dtype::from(ts))),
            (Some(ts), ast::VarDeclInner::Array(arr)) => Some(Ty::concrete(Dtype::array_of(
                Dtype::from(ts),
                arr.len,
            ))),
            (None, ast::VarDeclInner::Array(arr)) => Some(Ty::concrete(Dtype::array_of(
                Dtype::I32,
                arr.len,
            ))),
            (None, ast::VarDeclInner::Scalar) => None,
        };
        if let Some(ty) = ty {
            self.env.insert(decl.identifier.clone(), ty);
        }
    }

    // -----------------------------------------------------------------------
    // Variable definition (with initialiser)
    // -----------------------------------------------------------------------

    /// Translate `let x[: T] = e;` into an env binding, emitting a
    /// unification constraint when both a declared type and an
    /// initializer are present.
    ///
    /// Scalar case (`VarDefInner::Scalar`):
    /// - Compute `rhs: Ty` via `self.type_of_right_val(&scalar.val)`.
    /// - If a type was declared (`Some(t)`), build
    ///   `lhs = Ty::concrete(compose_var_def_dtype(t, &def.inner))`,
    ///   `unify(self.uf, &lhs, &rhs, &def.identifier)?`, and store
    ///   `lhs` in the env.
    /// - Otherwise, store `rhs` directly — `rhs` may still be a
    ///   `Ty::Var`, and that is the whole point (locals initialised
    ///   from pending calls carry the caller's α until it is resolved).
    ///
    /// Array case (`VarDefInner::Array`):
    /// - Element type defaults to `Dtype::I32` when omitted.
    /// - Build the concrete array `Ty` via `compose_var_def_dtype`, call
    ///   `self.check_array_initializer` for side-effects on any pending
    ///   callees inside the initializer, and drop the array `Ty` into
    ///   the env.  Arrays never hold a type variable themselves.
    #[allow(unused_variables)]
    fn process_var_def(&mut self, def: &ast::VarDef) -> Result<(), Error> {
        // TODO(asmt-2 §3.4 + §4.3): Implement variable definition.
        //
        // See the docstring above for the step-by-step recipe.  The key
        // educational point is: a declared type on the LHS becomes a
        // `unify(lhs, rhs)` constraint rather than a direct equality
        // check, which is the only change from `type_infer.rs`'s R2 rule.
        todo!("asmt-2: process_var_def")
    }

    fn check_array_initializer(&mut self, init: &ast::ArrayInitializer) -> Result<(), Error> {
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

    // -----------------------------------------------------------------------
    // Assignment
    // -----------------------------------------------------------------------

    fn process_assignment(&mut self, stmt: &ast::AssignmentStmt) -> Result<(), Error> {
        let rhs = self.type_of_right_val(&stmt.right_val)?;

        match &stmt.left_val.inner {
            ast::LeftValInner::Id(id) => {
                match self.env.get(id).cloned() {
                    Some(lhs) => unify(self.uf, &lhs, &rhs, id)?,
                    None => {
                        // Skip globals (always concrete; Pass 3 checks
                        // them); for a forward-flow-inferred local,
                        // publish the RHS as its type so later constraints
                        // can see it.
                        if !self.globals.contains_key(id.as_str()) {
                            self.env.insert(id.clone(), rhs);
                        }
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

    // -----------------------------------------------------------------------
    // Branching
    // -----------------------------------------------------------------------

    fn process_if(&mut self, stmt: &ast::IfStmt) -> Result<(), Error> {
        self.check_bool_unit(&stmt.bool_unit)?;

        // Both arms start from a snapshot of the pre-branch environment
        // so they cannot cross-contaminate.  Constraints they emit land
        // in the shared union-find regardless.
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

        self.merge_branches(&then_env, &else_env)
    }

    fn process_while(&mut self, stmt: &ast::WhileStmt) -> Result<(), Error> {
        self.check_bool_unit(&stmt.bool_unit)?;

        let mut body_ctx = self.fork(self.env.clone());
        body_ctx.process_stmts(&stmt.stmts)?;
        let body_env = body_ctx.env;

        self.merge_with_body(&body_env)
    }

    /// Unify the two branch environments back into `self.env`.
    ///
    /// Algorithm:
    /// - Iterate over every variable that already existed before the
    ///   branch (i.e. every name in the current `self.env`).
    /// - Look up that name in `env_a` and `env_b`; if a branch never
    ///   touched the variable, fall back to its pre-branch `Ty`.
    /// - `unify` the two `Ty`s so that a type learned on one side
    ///   flows to the other (and any conflict turns into
    ///   `Error::TypeMismatch`).
    /// - After unification, either side is a fine representative —
    ///   write it back into `self.env` so subsequent statements see
    ///   the merged type.
    ///
    /// Variables that were introduced *only* inside a branch go out
    /// of scope at the merge point; do not copy them into `self.env`.
    ///
    /// Mirrors `type_infer.rs::merge_envs`; the only structural change
    /// is "concrete equality" → "`unify` over `Ty`".
    #[allow(unused_variables)]
    fn merge_branches(
        &mut self,
        env_a: &HashMap<String, Ty>,
        env_b: &HashMap<String, Ty>,
    ) -> Result<(), Error> {
        // TODO(asmt-2 §3.4): Implement if/else merge via unify.
        todo!("asmt-2: merge_branches")
    }

    /// Unify a while-body environment back into `self.env`.
    ///
    /// A while loop may execute zero times, so whatever the body
    /// learned must remain compatible with the pre-loop state.
    /// Concretely: for every name already in `self.env`, `unify`
    /// its base `Ty` with whatever the body has for that name
    /// (fall back to the base `Ty` if the body didn't touch it).
    ///
    /// Mirrors `type_infer.rs::merge_env_single`.
    #[allow(unused_variables)]
    fn merge_with_body(&mut self, branch_env: &HashMap<String, Ty>) -> Result<(), Error> {
        // TODO(asmt-2 §3.4): Implement while-body merge via unify.
        todo!("asmt-2: merge_with_body")
    }

    // -----------------------------------------------------------------------
    // Return
    // -----------------------------------------------------------------------

    /// Emit the return-side constraint for one `return` statement.
    ///
    /// Algorithm:
    /// - Compute the `Ty` of the returned expression:
    ///   `Ty::Concrete(Dtype::Void)` for `return;`, otherwise
    ///   `self.type_of_right_val(val)?`.
    /// - If `self.return_var` is `Some(α_f)` (i.e. the enclosing
    ///   function declared no return type), feed the constraint
    ///   into α_f's class by calling
    ///   `unify(self.uf, &Ty::Var(α_f), &actual, self.fn_name)`.
    ///   Any conflict surfaces from `unify` as `TypeMismatch` —
    ///   that is what catches `type_infer_5`.
    /// - For functions with an explicit return type, `self.return_var`
    ///   is `None`.  Pass 3's forward-flow `type_infer.rs` handles the
    ///   compatibility check in that case, so there is nothing to do
    ///   here.
    #[allow(unused_variables)]
    fn process_return(&mut self, stmt: &ast::ReturnStmt) -> Result<(), Error> {
        // TODO(asmt-2 §4.3): Feed the return expression into α_f.
        todo!("asmt-2: process_return")
    }

    // -----------------------------------------------------------------------
    // Expression typing
    // -----------------------------------------------------------------------

    fn type_of_right_val(&mut self, val: &ast::RightVal) -> Result<Ty, Error> {
        match &val.inner {
            ast::RightValInner::ArithExpr(expr) => self.type_of_arith_expr(expr),
            ast::RightValInner::BoolExpr(expr) => {
                self.check_bool_expr(expr)?;
                Ok(Ty::concrete(Dtype::I32))
            }
        }
    }

    fn type_of_arith_expr(&mut self, expr: &ast::ArithExpr) -> Result<Ty, Error> {
        match &expr.inner {
            ast::ArithExprInner::ArithBiOpExpr(biop) => {
                self.type_of_arith_expr(&biop.left)?;
                self.type_of_arith_expr(&biop.right)?;
                // Arithmetic in TeaLang is always i32 -> i32 -> i32.  The
                // recursive walks let a pending-function call inside the
                // operands unify its α against i32.
                Ok(Ty::concrete(Dtype::I32))
            }
            ast::ArithExprInner::ExprUnit(unit) => self.type_of_expr_unit(unit),
        }
    }

    fn type_of_expr_unit(&mut self, unit: &ast::ExprUnit) -> Result<Ty, Error> {
        match &unit.inner {
            ast::ExprUnitInner::Num(_) => Ok(Ty::concrete(Dtype::I32)),
            ast::ExprUnitInner::Id(id) => self.resolve_variable(id),
            ast::ExprUnitInner::ArithExpr(expr) => self.type_of_arith_expr(expr),
            ast::ExprUnitInner::FnCall(call) => self.type_of_fn_call(call),
            ast::ExprUnitInner::ArrayExpr(expr) => self.type_of_array_expr(expr),
            ast::ExprUnitInner::MemberExpr(expr) => self.type_of_member_expr(expr),
            ast::ExprUnitInner::Reference(id) => self.type_of_reference(id),
        }
    }

    /// Look up `id` through the local env → globals chain.  The result
    /// may still be a type variable; known-but-unresolved locals are
    /// never an error here.
    fn lookup_ty(&self, id: &str) -> Result<Ty, Error> {
        if let Some(ty) = self.env.get(id) {
            return Ok(ty.clone());
        }
        match self.globals.get(id) {
            Some(gv) => Ok(Ty::concrete(gv.dtype.clone())),
            None => Err(Error::VariableNotDefined {
                symbol: id.to_string(),
            }),
        }
    }

    fn resolve_variable(&self, id: &str) -> Result<Ty, Error> {
        // Array identifiers decay to their element type.
        let ty = self.lookup_ty(id)?;
        Ok(match ty {
            Ty::Concrete(Dtype::Array { element, .. }) => Ty::concrete(element.as_ref().clone()),
            other => other,
        })
    }

    /// Type of a function call expression.
    ///
    /// Algorithm:
    /// - Walk every argument via `self.type_of_right_val(arg)?` so
    ///   that pending callees nested inside the arguments still
    ///   get a chance to unify.
    /// - Ask the AST for the qualified callee name
    ///   (`call.qualified_name()`).
    /// - If that name is in `self.pending`, return `Ty::Var(α_callee)`.
    ///   **This is the key step that makes self-recursion work** —
    ///   the recursive call to `pow` inside `pow`'s body sees α_pow,
    ///   and when the base-case `return 1;` pins α_pow to `i32` the
    ///   recursive call site's type is automatically resolved.
    /// - Otherwise look up the callee in `self.registry.function_types`
    ///   and wrap its `return_dtype` as `Ty::Concrete(...)`.  Undefined
    ///   callees are `Error::FunctionNotDefined`.
    #[allow(unused_variables)]
    fn type_of_fn_call(&mut self, call: &ast::FnCall) -> Result<Ty, Error> {
        // TODO(asmt-2 §4.3): Return Ty::Var for pending callees,
        // Ty::Concrete for registered ones.
        todo!("asmt-2: type_of_fn_call")
    }

    fn type_of_array_expr(&mut self, expr: &ast::ArrayExpr) -> Result<Ty, Error> {
        let arr_ty = self.type_of_left_val(&expr.arr)?;
        Ok(element_ty_of_indexing(&arr_ty))
    }

    fn type_of_member_expr(&mut self, expr: &ast::MemberExpr) -> Result<Ty, Error> {
        // Structs are always registered with concrete types, so member
        // access cannot involve a type variable.
        let struct_ty = self.type_of_left_val(&expr.struct_id)?;
        let struct_dtype = match struct_ty {
            Ty::Concrete(d) => d,
            Ty::Var(_) => {
                return Err(Error::InvalidStructMemberExpression { expr: expr.clone() });
            }
        };
        let type_name = struct_dtype
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
            .map(|(_, member)| Ty::concrete(member.dtype.clone()))
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })
    }

    fn type_of_reference(&self, id: &str) -> Result<Ty, Error> {
        let ty = self.lookup_ty(id)?;
        // Only arrays (or pointers to arrays) can be referenced; the
        // result is `*[element; ?]`.  Type variables never appear here
        // because parameter types are always explicit.
        let element = match ty {
            Ty::Concrete(Dtype::Array { element, .. }) => element.as_ref().clone(),
            Ty::Concrete(Dtype::Pointer { pointee }) => match *pointee {
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
        Ok(Ty::concrete(Dtype::ptr_to(Dtype::Array {
            element: Box::new(element),
            length: None,
        })))
    }

    fn type_of_left_val(&mut self, val: &ast::LeftVal) -> Result<Ty, Error> {
        match &val.inner {
            ast::LeftValInner::Id(id) => self.lookup_ty(id),
            ast::LeftValInner::ArrayExpr(expr) => self.type_of_left_val_array(expr),
            ast::LeftValInner::MemberExpr(expr) => self.type_of_member_expr(expr),
        }
    }

    fn type_of_left_val_array(&mut self, expr: &ast::ArrayExpr) -> Result<Ty, Error> {
        let arr_ty = self.type_of_left_val(&expr.arr)?;
        Ok(element_ty_of_indexing(&arr_ty))
    }

    // -----------------------------------------------------------------------
    // Boolean expressions (only walked for side-effects on call sites)
    // -----------------------------------------------------------------------

    fn check_bool_expr(&mut self, expr: &ast::BoolExpr) -> Result<(), Error> {
        match &expr.inner {
            ast::BoolExprInner::BoolBiOpExpr(biop) => {
                self.check_bool_expr(&biop.left)?;
                self.check_bool_expr(&biop.right)
            }
            ast::BoolExprInner::BoolUnit(unit) => self.check_bool_unit(unit),
        }
    }

    fn check_bool_unit(&mut self, unit: &ast::BoolUnit) -> Result<(), Error> {
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
}

/// Array-decay on indexing: `Array<T, _>` and `Pointer<Array<T, _>>`
/// yield `T`.  Array positions are always concrete (parameters and
/// local arrays never hold a type variable), so we only need to match
/// against `Ty::Concrete`.
fn element_ty_of_indexing(ty: &Ty) -> Ty {
    match ty {
        Ty::Concrete(Dtype::Array { element, .. }) => Ty::concrete(element.as_ref().clone()),
        Ty::Concrete(Dtype::Pointer { pointee }) => match pointee.as_ref() {
            Dtype::Array { element, .. } => Ty::concrete(element.as_ref().clone()),
            _ => ty.clone(),
        },
        _ => ty.clone(),
    }
}
