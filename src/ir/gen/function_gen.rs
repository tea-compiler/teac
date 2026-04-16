//! IR generation for function bodies.
//!
//! This module translates an AST function definition ([`ast::FnDef`]) into a
//! flat sequence of IR statements by walking each statement and expression node.
//! It handles local variable allocation, control-flow (if / while / break /
//! continue), arithmetic and boolean expressions, array and struct member
//! access, and function calls.

use crate::ast::{self, ArrayInitializer, AssignmentStmt, RightValList};
use crate::ir::function::{BlockLabel, FunctionGenerator};
use crate::ir::gen::conversions::{compose_var_decl_dtype, compose_var_def_dtype};
use crate::ir::stmt::{ArithBinOp, CmpPredicate, StmtInner};
use crate::ir::types::Dtype;
use crate::ir::value::{Local, Operand};
use crate::ir::Error;

/// Builds an i32-typed [`Operand`] for a GEP index.
///
/// Array/struct indices are `usize` in the AST and source-language domain
/// but must be lowered to `i32` to match LLVM IR's GEP index width.  The
/// compiler panics if a single declared array or struct exceeds `i32::MAX`
/// elements — this is a hard limit on the emitted IR, not a TeaLang rule,
/// and in practice no program will ever approach it.
fn array_index_operand(index: usize) -> Operand {
    Operand::from(
        i32::try_from(index).expect("array/struct index exceeds i32::MAX (LLVM GEP width)"),
    )
}

// ── Function entry-point generation ──────────────────────────────────────────

impl FunctionGenerator<'_> {
    /// Generates IR for a complete function definition.
    ///
    /// Emits the function entry label, allocates stack slots for every argument
    /// (alloca + store pattern), lowers each statement in the body, and appends
    /// an implicit return if the last instruction is not already a `return`.
    ///
    /// # Parameters
    /// - `from`: the AST node for the function being compiled.
    ///
    /// # Errors
    /// Returns an error if the function is not registered in the type registry,
    /// an argument name is redefined, or the return type is unsupported.
    pub fn generate(&mut self, from: &ast::FnDef) -> Result<(), Error> {
        let identifier = &from.fn_decl.identifier;
        let function_type = self
            .registry
            .function_types
            .get(identifier)
            .ok_or_else(|| Error::FunctionNotDefined {
                symbol: identifier.clone(),
            })?;

        let arguments = function_type.arguments.clone();
        let return_dtype = function_type.return_dtype.clone();
        self.emit_label(BlockLabel::Function(identifier.clone()));

        // Spill every argument to the stack (alloca + store) so they are addressable.
        for (id, dtype) in &arguments {
            if self.local_variables.contains_key(id) {
                return Err(Error::VariableRedefinition { symbol: id.clone() });
            }

            // Allocate a virtual register that carries the incoming argument value.
            let arg_local = self.fresh_local(dtype.clone());
            self.arguments.push(arg_local.clone());

            // Allocate a stack slot (pointer to the argument type) for the argument.
            let slot = self.fresh_local(Dtype::ptr_to(dtype.clone()));
            self.emit_alloca(Operand::from(&slot));
            // Store the incoming value into the newly allocated stack slot.
            self.emit_store(Operand::from(arg_local), Operand::from(&slot));
            self.local_variables.insert(id.clone(), slot);
        }

        // Lower the function body statement by statement.
        for stmt in &from.stmts {
            self.handle_block(stmt, None, None)?;
        }

        // Append an implicit return if the last instruction is not already a
        // return.  `FunctionType::try_from` whitelists return types to
        // Void/I32 at registration, so any other variant here would be a
        // broken invariant in the front-end.
        if let Some(stmt) = self.irs.last() {
            if !matches!(stmt.inner, StmtInner::Return(_)) {
                match &return_dtype {
                    Dtype::I32 => self.emit_return(Some(Operand::from(0))),
                    Dtype::Void => self.emit_return(None),
                    other => unreachable!(
                        "function {} has return type {other} which \
                         FunctionType::try_from should have rejected",
                        identifier
                    ),
                }
            }
        }

        Ok(())
    }
}

// ── Statement handlers ────────────────────────────────────────────────────────

impl FunctionGenerator<'_> {
    /// Dispatches a single code-block statement to the appropriate handler.
    ///
    /// `con_label` and `bre_label` are the jump targets for `continue` and
    /// `break` inside the current loop, respectively.  Both are `None` when the
    /// statement is not nested inside a loop.
    pub fn handle_block(
        &mut self,
        stmt: &ast::CodeBlockStmt,
        con_label: Option<&BlockLabel>,
        bre_label: Option<&BlockLabel>,
    ) -> Result<(), Error> {
        match &stmt.inner {
            ast::CodeBlockStmtInner::Assignment(s) => self.handle_assignment_stmt(s),
            ast::CodeBlockStmtInner::VarDecl(s) => match &s.inner {
                ast::VarDeclStmtInner::Decl(d) => self.handle_local_var_decl(d),
                ast::VarDeclStmtInner::Def(d) => self.handle_local_var_def(d),
            },
            ast::CodeBlockStmtInner::Call(s) => self.handle_call_stmt(s),
            ast::CodeBlockStmtInner::If(s) => self.handle_if_stmt(s, con_label, bre_label),
            ast::CodeBlockStmtInner::While(s) => self.handle_while_stmt(s),
            ast::CodeBlockStmtInner::Return(s) => self.handle_return_stmt(s),
            ast::CodeBlockStmtInner::Continue(_) => self.handle_continue_stmt(con_label),
            ast::CodeBlockStmtInner::Break(_) => self.handle_break_stmt(bre_label),
            ast::CodeBlockStmtInner::Null(_) => Ok(()),
        }
    }

    /// Lowers an assignment statement (`left = right`).
    ///
    /// `handle_left_val` yields a pointer to the destination's stack slot and
    /// `handle_right_val` yields the value to store, so the assignment is a
    /// single `store` instruction.
    pub fn handle_assignment_stmt(&mut self, stmt: &AssignmentStmt) -> Result<(), Error> {
        let left = self.handle_left_val(&stmt.left_val)?;
        let right = self.handle_right_val(&stmt.right_val)?;
        self.emit_store(right, left);
        Ok(())
    }

    /// Inserts a local variable into the current scope's symbol table.
    ///
    /// Records the identifier for scope-exit cleanup via [`record_scoped_local`].
    /// Returns `VariableRedefinition` if a variable with the same name already
    /// exists in the symbol table.
    fn insert_scoped_local(
        &mut self,
        identifier: &str,
        variable: Local,
    ) -> Result<(), Error> {
        if self
            .local_variables
            .insert(identifier.to_string(), variable)
            .is_some()
        {
            return Err(Error::VariableRedefinition {
                symbol: identifier.to_string(),
            });
        }
        self.record_scoped_local(identifier.to_string());
        Ok(())
    }

    /// Creates a new pointer-typed local and emits an `alloca` for it.
    ///
    /// Returns the resulting [`Local`] whose type is `*pointee`.
    fn allocate_pointer_local(&mut self, pointee: Dtype) -> Local {
        let local = self.fresh_local(Dtype::ptr_to(pointee));
        self.emit_alloca(Operand::from(&local));
        local
    }

    /// Allocates stack space for a scalar local and initializes it with `right_val`.
    ///
    /// Combines [`allocate_pointer_local`] with an immediate `store` instruction.
    fn define_scalar_local(&mut self, pointee: Dtype, right_val: Operand) -> Local {
        let local = self.allocate_pointer_local(pointee);
        self.emit_store(right_val, Operand::from(&local));
        local
    }

    /// Resolves the element/scalar type of a local variable.
    ///
    /// For typed declarations the base comes directly from the AST annotation.
    /// For **untyped scalars**, the base comes from the resolved-types map
    /// produced by the type inference pass (falling back to `i32` when
    /// inference could not determine anything — e.g. a declared-but-never-used
    /// local).  **Untyped arrays** always default to `i32` elements because
    /// TeaLang does not support inferring an array's element type.
    fn local_base_dtype(
        &self,
        identifier: &str,
        explicit: Option<&Dtype>,
        is_scalar: bool,
    ) -> Dtype {
        match (explicit, is_scalar) {
            (Some(t), _) => t.clone(),
            (None, true) => self
                .resolved_types
                .get(identifier)
                .cloned()
                .unwrap_or(Dtype::I32),
            (None, false) => Dtype::I32,
        }
    }

    /// Determines the storage type for a local variable *declaration*
    /// (no initialiser).
    fn plan_local_decl_storage(&self, decl: &ast::VarDecl) -> Dtype {
        let explicit = decl.type_specifier.as_ref().map(Dtype::from);
        let is_scalar = matches!(&decl.inner, ast::VarDeclInner::Scalar);
        let base = self.local_base_dtype(&decl.identifier, explicit.as_ref(), is_scalar);
        compose_var_decl_dtype(base, &decl.inner)
    }

    /// Lowers a local variable declaration (without an initialiser) by
    /// allocating a stack slot of the declaration's storage type and
    /// inserting it into the current scope's symbol table.
    pub fn handle_local_var_decl(&mut self, decl: &ast::VarDecl) -> Result<(), Error> {
        let identifier = decl.identifier.as_str();
        let pointee = self.plan_local_decl_storage(decl);
        let variable = self.allocate_pointer_local(pointee);
        self.insert_scoped_local(identifier, variable)
    }

    /// Stores a flat list of values into a stack-allocated array.
    ///
    /// For each value, computes an element pointer via GEP using the value's
    /// position as the index and emits a `store` instruction.
    ///
    /// # Parameters
    /// - `base_ptr`: operand pointing to the first element of the array.
    /// - `vals`: list of right-hand-side values to store sequentially.
    pub fn init_array(&mut self, base_ptr: &Operand, vals: &RightValList) -> Result<(), Error> {
        for (i, val) in vals.iter().enumerate() {
            let element_ptr = Operand::from(self.fresh_local(Dtype::ptr_to(Dtype::I32)));
            let right_elem = self.handle_right_val(val)?;

            self.emit_gep(element_ptr.clone(), base_ptr.clone(), array_index_operand(i));
            self.emit_store(right_elem, element_ptr);
        }
        Ok(())
    }

    /// Initializes an array from an [`ArrayInitializer`].
    ///
    /// Delegates to [`init_array`] for explicit element lists.  For fill
    /// initializers, evaluates the fill value once and repeats the store for
    /// every index up to `count`.
    pub fn init_array_from(
        &mut self,
        base_ptr: &Operand,
        initializer: &ArrayInitializer,
    ) -> Result<(), Error> {
        match initializer {
            ArrayInitializer::ExplicitList(vals) => self.init_array(base_ptr, vals),
            ArrayInitializer::Fill { val, count } => {
                let fill_val = self.handle_right_val(val)?;
                for i in 0..*count {
                    let element_ptr = Operand::from(self.fresh_local(Dtype::ptr_to(Dtype::I32)));
                    self.emit_gep(element_ptr.clone(), base_ptr.clone(), array_index_operand(i));
                    self.emit_store(fill_val.clone(), element_ptr);
                }
                Ok(())
            }
        }
    }

    /// Lowers a local variable definition (declaration with an initializer)
    /// by allocating a stack slot and storing the initial value into it.
    pub fn handle_local_var_def(&mut self, def: &ast::VarDef) -> Result<(), Error> {
        let identifier = def.identifier.as_str();
        let explicit = def.type_specifier.as_ref().map(Dtype::from);
        let is_scalar = matches!(&def.inner, ast::VarDefInner::Scalar(_));
        let base = self.local_base_dtype(identifier, explicit.as_ref(), is_scalar);
        let pointee = compose_var_def_dtype(base, &def.inner);

        let variable: Local = match &def.inner {
            ast::VarDefInner::Scalar(scalar) => {
                let right_val = self.handle_right_val(&scalar.val)?;
                self.define_scalar_local(pointee, right_val)
            }
            ast::VarDefInner::Array(array) => {
                let local = self.allocate_pointer_local(pointee);
                self.init_array_from(&Operand::from(&local), &array.initializer)?;
                local
            }
        };

        self.insert_scoped_local(identifier, variable)
    }

    /// Lowers a standalone function call statement.
    ///
    /// Evaluates each argument, allocates a temporary for a non-void return
    /// value (which is subsequently discarded), and emits the `call` instruction.
    pub fn handle_call_stmt(&mut self, stmt: &ast::CallStmt) -> Result<(), Error> {
        let function_name = stmt.fn_call.qualified_name();
        let mut args = Vec::new();
        for arg in &stmt.fn_call.vals {
            let right_val = self.handle_right_val(arg)?;
            args.push(right_val);
        }

        match self.registry.function_types.get(&function_name) {
            None => Err(Error::FunctionNotDefined {
                symbol: function_name,
            }),
            Some(function_type) => {
                // `FunctionType::try_from` whitelists return types to Void/I32
                // at registration; any other variant here would indicate a
                // broken invariant in the front-end.
                let retval = match &function_type.return_dtype {
                    Dtype::Void => None,
                    Dtype::I32 => Some(Operand::from(self.fresh_local(Dtype::I32))),
                    other => unreachable!(
                        "registered function {function_name} has return type {other} \
                         which FunctionType::try_from should have rejected"
                    ),
                };
                self.emit_call(function_name, retval, args);
                Ok(())
            }
        }
    }

    /// Lowers an `if` / `else` statement into branching IR.
    ///
    /// Allocates three basic blocks (`true_label`, `false_label`, `after_label`)
    /// and emits a conditional branch on the boolean condition.  Both the
    /// then-branch and the (possibly absent) else-branch jump to `after_label`
    /// when they finish.
    ///
    /// `con_label` and `bre_label` are threaded through to nested statements so
    /// that `continue` / `break` inside the branches target the correct loop.
    pub fn handle_if_stmt(
        &mut self,
        stmt: &ast::IfStmt,
        con_label: Option<&BlockLabel>,
        bre_label: Option<&BlockLabel>,
    ) -> Result<(), Error> {
        let true_label = self.alloc_basic_block();
        let false_label = self.alloc_basic_block();
        let after_label = self.alloc_basic_block();

        // Evaluate the condition; jump to the appropriate branch.
        self.handle_bool_unit(&stmt.bool_unit, true_label.clone(), false_label.clone())?;

        // Emit the then-branch; a new scope is opened so that any locals are cleaned up.
        self.emit_label(true_label);
        self.enter_scope();
        for s in &stmt.if_stmts {
            self.handle_block(s, con_label, bre_label)?;
        }
        self.exit_scope();
        // Jump past the else-branch to the merge point.
        self.emit_jump(after_label.clone());

        // Emit the (possibly absent) else-branch in its own scope.
        self.emit_label(false_label);
        self.enter_scope();
        if let Some(else_stmts) = &stmt.else_stmts {
            for s in else_stmts {
                self.handle_block(s, con_label, bre_label)?;
            }
        }
        self.exit_scope();
        self.emit_jump(after_label.clone());

        // Merge point reached by both branches.
        self.emit_label(after_label);

        Ok(())
    }

    /// Lowers a `while` loop into branching IR.
    ///
    /// Structure:
    /// ```text
    ///   entry → test_label ←── back-edge
    ///                ↓ true        ↓ false
    ///           true_label    false_label
    /// ```
    /// `continue` inside the body targets `test_label`; `break` targets `false_label`.
    pub fn handle_while_stmt(&mut self, stmt: &ast::WhileStmt) -> Result<(), Error> {
        let test_label = self.alloc_basic_block();
        let true_label = self.alloc_basic_block();
        let false_label = self.alloc_basic_block();

        // Jump unconditionally into the loop test from the predecessor block.
        self.emit_jump(test_label.clone());

        // Emit the loop condition test.
        self.emit_label(test_label.clone());
        self.handle_bool_unit(&stmt.bool_unit, true_label.clone(), false_label.clone())?;

        // Loop body; `continue` → test_label, `break` → false_label.
        self.emit_label(true_label);
        self.enter_scope();
        for s in &stmt.stmts {
            self.handle_block(s, Some(&test_label), Some(&false_label))?;
        }
        self.exit_scope();
        // Back-edge: jump back to the loop condition.
        self.emit_jump(test_label);

        self.emit_label(false_label);
        Ok(())
    }

    /// Lowers a `return` statement.
    ///
    /// Emits a void `return` when no value is present, or evaluates the return
    /// expression and emits a value-carrying `return` otherwise.
    pub fn handle_return_stmt(&mut self, stmt: &ast::ReturnStmt) -> Result<(), Error> {
        match &stmt.val {
            None => {
                self.emit_return(None);
            }
            Some(val) => {
                let val = self.handle_right_val(val)?;
                self.emit_return(Some(val));
            }
        }
        Ok(())
    }

    /// Lowers a `continue` statement by jumping to the enclosing loop's test label.
    ///
    /// Returns `InvalidContinueInst` if called outside of a loop context.
    pub fn handle_continue_stmt(&mut self, con_label: Option<&BlockLabel>) -> Result<(), Error> {
        let label = con_label.ok_or(Error::InvalidContinueInst)?;
        self.emit_jump(label.clone());
        Ok(())
    }

    /// Lowers a `break` statement by jumping to the enclosing loop's exit label.
    ///
    /// Returns `InvalidBreakInst` if called outside of a loop context.
    pub fn handle_break_stmt(&mut self, bre_label: Option<&BlockLabel>) -> Result<(), Error> {
        let label = bre_label.ok_or(Error::InvalidBreakInst)?;
        self.emit_jump(label.clone());
        Ok(())
    }
}

// ── Expression and value handlers ─────────────────────────────────────────────

impl FunctionGenerator<'_> {
    /// Lowers a comparison expression into a conditional branch.
    ///
    /// Emits a `cmp` instruction (result type `i1`) followed by a conditional
    /// jump to `true_label` or `false_label`.
    fn handle_com_op_expr(
        &mut self,
        expr: &ast::ComExpr,
        true_label: BlockLabel,
        false_label: BlockLabel,
    ) -> Result<(), Error> {
        let left = self.handle_expr_unit(&expr.left)?;
        let right = self.handle_expr_unit(&expr.right)?;

        let dst = Operand::from(self.fresh_local(Dtype::I1));
        self.emit_cmp(
            CmpPredicate::from(&expr.op),
            left,
            right,
            dst.clone(),
        );
        self.emit_cjump(dst, true_label, false_label);

        Ok(())
    }

    /// Lowers a single expression unit to an [`Operand`].
    ///
    /// After resolving the unit's inner form, performs an implicit load for
    /// addressable scalar pointers and for global `i32` values, so the caller
    /// always receives a value-typed operand rather than a pointer.
    fn handle_expr_unit(&mut self, unit: &ast::ExprUnit) -> Result<Operand, Error> {
        let operand = match &unit.inner {
            ast::ExprUnitInner::Num(num) => Ok(Operand::from(*num)),
            ast::ExprUnitInner::Id(id) => {
                let op = self.lookup_variable(id)?;
                // Arrays cannot be used directly as scalar values.
                let is_array = matches!(
                    op.dtype(),
                    Dtype::Pointer { pointee } if matches!(pointee.as_ref(), Dtype::Array { .. })
                ) || matches!(op.dtype(), Dtype::Array { .. });
                if is_array {
                    return Err(Error::ArrayUsedAsValue { symbol: id.clone() });
                }
                Ok(op)
            }
            ast::ExprUnitInner::ArithExpr(expr) => self.handle_arith_expr(expr),
            ast::ExprUnitInner::FnCall(fn_call) => {
                let name = fn_call.qualified_name();
                let return_dtype = &self
                    .registry
                    .function_types
                    .get(&name)
                    .ok_or_else(|| Error::InvalidExprUnit {
                        expr_unit: unit.clone(),
                    })?
                    .return_dtype;

                // `FunctionType::try_from` whitelists return types to Void/I32.
                // In expression position, only I32 is usable; a void call in
                // an expression is a source-level mistake.
                let res = match return_dtype {
                    Dtype::I32 => Operand::from(self.fresh_local(Dtype::I32)),
                    Dtype::Void => {
                        return Err(Error::InvalidExprUnit {
                            expr_unit: unit.clone(),
                        });
                    }
                    other => unreachable!(
                        "registered function {name} has return type {other} \
                         which FunctionType::try_from should have rejected"
                    ),
                };

                let mut args: Vec<Operand> = Vec::new();
                for arg in &fn_call.vals {
                    let rval = self.handle_right_val(arg)?;
                    args.push(rval);
                }
                self.emit_call(name, Some(res.clone()), args);

                Ok(res)
            }
            ast::ExprUnitInner::ArrayExpr(expr) => self.handle_array_expr(expr),
            ast::ExprUnitInner::MemberExpr(expr) => self.handle_member_expr(expr),
            ast::ExprUnitInner::Reference(id) => {
                return self.handle_reference_expr(id);
            }
        }?;

        Ok(match operand.dtype() {
            // Auto-load: dereference addressable scalar pointers (but leave arrays/structs as-is).
            Dtype::Pointer { pointee }
                if operand.is_addressable()
                    && !matches!(pointee.as_ref(), Dtype::Array { .. } | Dtype::Struct { .. }) =>
            {
                let dst = Operand::from(self.fresh_local(pointee.as_ref().clone()));
                self.emit_load(dst.clone(), operand);
                dst
            }
            // Auto-load global i32 values which are stored behind a pointer.
            Dtype::I32 if matches!(&operand, Operand::Global(_)) => {
                let dst = Operand::from(self.fresh_local(Dtype::I32));
                self.emit_load(dst.clone(), operand);
                dst
            }
            _ => operand,
        })
    }

    /// Lowers a reference expression (`&id`) to a pointer to the array's first element.
    ///
    /// The variable must be (or point to) an array; emits a GEP with index 0 to
    /// yield a `*[element_type; ?]` operand.
    fn handle_reference_expr(&mut self, id: &str) -> Result<Operand, Error> {
        let operand = self.lookup_variable(id)?;
        let element_type = match operand.dtype() {
            Dtype::Pointer { pointee } => match pointee.as_ref() {
                Dtype::Array { element, .. } => element.as_ref().clone(),
                _ => {
                    return Err(Error::InvalidReference {
                        symbol: id.to_string(),
                    });
                }
            },
            Dtype::Array { element, .. } => element.as_ref().clone(),
            _ => {
                return Err(Error::InvalidReference {
                    symbol: id.to_string(),
                });
            }
        };
        let target = Operand::from(self.fresh_local(Dtype::ptr_to(Dtype::Array {
            element: Box::new(element_type),
            length: None,
        })));
        self.emit_gep(target.clone(), operand, Operand::from(0i32));
        Ok(target)
    }

    /// Lowers an arithmetic expression (binary operation or a single unit).
    fn handle_arith_expr(&mut self, expr: &ast::ArithExpr) -> Result<Operand, Error> {
        match &expr.inner {
            ast::ArithExprInner::ArithBiOpExpr(expr) => self.handle_arith_biop_expr(expr),
            ast::ArithExprInner::ExprUnit(unit) => self.handle_expr_unit(unit),
        }
    }

    /// Lowers a right-hand-side value (arithmetic or boolean expression) to an [`Operand`].
    fn handle_right_val(&mut self, val: &ast::RightVal) -> Result<Operand, Error> {
        match &val.inner {
            ast::RightValInner::ArithExpr(expr) => self.handle_arith_expr(expr),
            ast::RightValInner::BoolExpr(expr) => self.handle_bool_expr_as_value(expr),
        }
    }

    /// Lowers an array element access expression (`arr[idx]`) to an element pointer.
    ///
    /// Loads the base pointer if it is itself pointer-typed (e.g., a parameter
    /// passed as a pointer-to-pointer), then computes the element address via GEP.
    fn handle_array_expr(&mut self, expr: &ast::ArrayExpr) -> Result<Operand, Error> {
        let arr = self.handle_left_val(&expr.arr)?;

        // If the array is accessed through a pointer-to-pointer (e.g., a function parameter
        // holding a pointer to an array), load the inner pointer first.
        let (arr, arr_dtype) = match arr.dtype() {
            Dtype::Pointer { pointee } if matches!(pointee.as_ref(), Dtype::Pointer { .. }) => {
                let loaded = Operand::from(self.fresh_local(pointee.as_ref().clone()));
                self.emit_load(loaded.clone(), arr);
                (loaded.clone(), loaded.dtype().clone())
            }
            _ => (arr.clone(), arr.dtype().clone()),
        };

        let target = match &arr_dtype {
            Dtype::Pointer { pointee } => match pointee.as_ref() {
                Dtype::Array { element, .. } => Ok(Operand::from(
                    self.fresh_local(Dtype::ptr_to(element.as_ref().clone())),
                )),
                _ => Ok(Operand::from(
                    self.fresh_local(Dtype::ptr_to(pointee.as_ref().clone())),
                )),
            },
            Dtype::Array { element, .. } => Ok(Operand::from(
                self.fresh_local(Dtype::ptr_to(element.as_ref().clone())),
            )),
            _ => Err(Error::InvalidArrayExpression),
        }?;

        let index = self.handle_index_expr(expr.idx.as_ref())?;
        self.emit_gep(target.clone(), arr, index);

        Ok(target)
    }

    /// Lowers a struct member access expression (`s.member`) to a member pointer.
    ///
    /// Looks up the struct type in the registry, finds the member's field index,
    /// and emits a GEP to yield a pointer to that member.
    fn handle_member_expr(&mut self, expr: &ast::MemberExpr) -> Result<Operand, Error> {
        let s = self.handle_left_val(&expr.struct_id)?;

        let type_name = s
            .dtype()
            .struct_type_name()
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })?;

        let struct_type = self
            .registry
            .struct_types
            .get(type_name)
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })?;
        let member = struct_type
            .elements
            .iter()
            .find(|elem| elem.0 == expr.member_id)
            .map(|elem| &elem.1)
            .ok_or_else(|| Error::InvalidStructMemberExpression { expr: expr.clone() })?;
        let member_dtype = member.dtype.clone();
        let member_index = i32::try_from(member.index).map_err(|_| {
            Error::InvalidStructMemberExpression { expr: expr.clone() }
        })?;

        let target = match &member_dtype {
            Dtype::Void => {
                return Err(Error::InvalidStructMemberExpression { expr: expr.clone() })
            }
            _ => Operand::from(self.fresh_local(Dtype::ptr_to(member_dtype))),
        };

        self.emit_gep(target.clone(), s, Operand::from(member_index));
        Ok(target)
    }

    /// Resolves a left-hand-side value to an addressable [`Operand`] (a pointer).
    ///
    /// For a simple identifier, looks up the symbol; for array and member
    /// expressions, delegates to the respective handlers.
    fn handle_left_val(&mut self, val: &ast::LeftVal) -> Result<Operand, Error> {
        match &val.inner {
            ast::LeftValInner::Id(id) => self.lookup_variable(id),
            ast::LeftValInner::ArrayExpr(expr) => self.handle_array_expr(expr),
            ast::LeftValInner::MemberExpr(expr) => self.handle_member_expr(expr),
        }
    }

    /// Lowers a binary arithmetic expression (`left op right`) to an `i32` temporary.
    fn handle_arith_biop_expr(&mut self, expr: &ast::ArithBiOpExpr) -> Result<Operand, Error> {
        let left = self.handle_arith_expr(&expr.left)?;
        let right = self.handle_arith_expr(&expr.right)?;
        let dst = Operand::from(self.fresh_local(Dtype::I32));
        self.emit_biop(ArithBinOp::from(&expr.op), left, right, dst.clone());
        Ok(dst)
    }

    /// Lowers an array index expression to an `i32` operand.
    ///
    /// Variable indices are loaded from their stack slot; numeric literals are
    /// returned directly as immediate operands.
    fn handle_index_expr(&mut self, expr: &ast::IndexExpr) -> Result<Operand, Error> {
        match &expr.inner {
            ast::IndexExprInner::Id(id) => {
                let src = self.lookup_variable(id)?;
                let idx = Operand::from(self.fresh_local(Dtype::I32));
                self.emit_load(idx.clone(), src);
                Ok(idx)
            }
            ast::IndexExprInner::Num(num) => Ok(array_index_operand(*num)),
        }
    }
}

// ── Boolean expression handlers ───────────────────────────────────────────────

impl FunctionGenerator<'_> {
    /// Lowers a boolean expression to a materialized `i32` value (0 or 1).
    ///
    /// Allocates a temporary `i32` stack slot, evaluates the expression as a
    /// branch (writing 1 on the true path and 0 on the false path via
    /// [`emit_bool_materialization`]), then loads and returns the result.
    fn handle_bool_expr_as_value(&mut self, expr: &ast::BoolExpr) -> Result<Operand, Error> {
        let true_label = self.alloc_basic_block();
        let false_label = self.alloc_basic_block();
        let after_label = self.alloc_basic_block();

        // Allocate stack storage for the materialised boolean result.
        let bool_evaluated = Operand::from(self.fresh_local(Dtype::ptr_to(Dtype::I32)));
        self.emit_alloca(bool_evaluated.clone());

        // Branch-based evaluation; result is written into bool_evaluated.
        self.handle_bool_expr_as_branch(expr, true_label.clone(), false_label.clone())?;
        self.emit_bool_materialization(
            true_label,
            false_label,
            after_label,
            bool_evaluated.clone(),
        );

        // Load the materialised 0/1 value back into a register.
        let loaded = Operand::from(self.fresh_local(Dtype::I32));
        self.emit_load(loaded.clone(), bool_evaluated);

        Ok(loaded)
    }

    /// Lowers a boolean expression as a branching construct.
    ///
    /// Jumps to `true_label` if the expression evaluates to true, or to
    /// `false_label` otherwise.
    fn handle_bool_expr_as_branch(
        &mut self,
        expr: &ast::BoolExpr,
        true_label: BlockLabel,
        false_label: BlockLabel,
    ) -> Result<(), Error> {
        match &expr.inner {
            ast::BoolExprInner::BoolBiOpExpr(biop) => {
                self.handle_bool_biop_expr(biop, true_label, false_label)
            }
            ast::BoolExprInner::BoolUnit(unit) => {
                self.handle_bool_unit(unit, true_label, false_label)
            }
        }
    }

    /// Emits the true/false branches that write an integer 0 or 1 into `bool_ptr`.
    ///
    /// - True path: stores 1 and jumps to `after_label`.
    /// - False path: stores 0 and jumps to `after_label`.
    ///
    /// Finishes by emitting `after_label` as the merge point.
    fn emit_bool_materialization(
        &mut self,
        true_label: BlockLabel,
        false_label: BlockLabel,
        after_label: BlockLabel,
        bool_ptr: Operand,
    ) {
        // True path: store 1 and jump to the merge point.
        self.emit_label(true_label);
        self.emit_store(Operand::from(1), bool_ptr.clone());
        self.emit_jump(after_label.clone());

        // False path: store 0 and jump to the merge point.
        self.emit_label(false_label);
        self.emit_store(Operand::from(0), bool_ptr);
        self.emit_jump(after_label.clone());

        self.emit_label(after_label);
    }

    /// Lowers a binary boolean expression (`&&` or `||`) using short-circuit evaluation.
    ///
    /// For `&&`: evaluate the left operand; jump to `false_label` immediately if
    /// false, otherwise fall through to evaluate the right operand.
    ///
    /// For `||`: evaluate the left operand; jump to `true_label` immediately if
    /// true, otherwise fall through to evaluate the right operand.
    fn handle_bool_biop_expr(
        &mut self,
        expr: &ast::BoolBiOpExpr,
        true_label: BlockLabel,
        false_label: BlockLabel,
    ) -> Result<(), Error> {
        let eval_right_label = self.alloc_basic_block();
        match &expr.op {
            ast::BoolBiOp::And => {
                // Short-circuit AND: only evaluate the right side if the left side is true.
                self.handle_bool_expr_as_branch(
                    &expr.left,
                    eval_right_label.clone(),
                    false_label.clone(),
                )?;
                self.emit_label(eval_right_label);

                self.handle_bool_expr_as_branch(&expr.right, true_label, false_label)?;
            }
            ast::BoolBiOp::Or => {
                // Short-circuit OR: only evaluate the right side if the left side is false.
                self.handle_bool_expr_as_branch(
                    &expr.left,
                    true_label.clone(),
                    eval_right_label.clone(),
                )?;
                self.emit_label(eval_right_label);

                self.handle_bool_expr_as_branch(&expr.right, true_label, false_label)?;
            }
        }
        Ok(())
    }

    /// Lowers a boolean unit (comparison, sub-expression, or negation) as a branch.
    ///
    /// For a negation (`!expr`), the true and false labels are swapped so that
    /// the inner expression's result is inverted.
    fn handle_bool_unit(
        &mut self,
        unit: &ast::BoolUnit,
        true_label: BlockLabel,
        false_label: BlockLabel,
    ) -> Result<(), Error> {
        match &unit.inner {
            ast::BoolUnitInner::ComExpr(expr) => {
                self.handle_com_op_expr(expr, true_label, false_label)
            }
            ast::BoolUnitInner::BoolExpr(expr) => {
                self.handle_bool_expr_as_branch(expr, true_label, false_label)
            }
            ast::BoolUnitInner::BoolUOpExpr(expr) => {
                self.handle_bool_unit(&expr.cond, false_label, true_label)
            }
        }
    }
}
