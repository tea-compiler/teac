//! IR generation for function bodies.
//!
//! This module translates an AST function definition ([`ast::FnDef`]) into a
//! flat sequence of IR statements by walking each statement and expression node.
//! It handles local variable allocation, control-flow (if / while / break /
//! continue), arithmetic and boolean expressions, array and struct member
//! access, and function calls.

use crate::ast::{self, ArrayInitializer, AssignmentStmt, RightValList};
use crate::ir::function::{BlockLabel, FunctionGenerator};
use crate::ir::stmt::{ArithBinOp, CmpPredicate, StmtInner};
use crate::ir::types::Dtype;
use crate::ir::value::{LocalVariable, Operand};
use crate::ir::Error;

/// Describes how stack storage for a local variable should be handled during IR generation.
enum LocalStoragePlan {
    /// Storage is deferred: the variable's type will be inferred from its first assignment.
    Deferred,
    /// Emit an `alloca` instruction immediately for the given element type.
    Alloca(Dtype),
}

// ── Function entry-point generation ──────────────────────────────────────────

impl<'ir> FunctionGenerator<'ir> {
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
        // Emit the function entry label.
        self.emit_label(BlockLabel::Function(identifier.clone()));

        // Spill every argument to the stack (alloca + store) so they are addressable.
        for (id, dtype) in arguments.iter() {
            if self.local_variables.contains_key(id) {
                return Err(Error::VariableRedefinition { symbol: id.clone() });
            }

            // Allocate a virtual register that carries the incoming argument value.
            let var = LocalVariable::new(dtype.clone(), self.alloc_vreg(), Some(id.to_string()));
            self.arguments.push(var.clone());

            // Allocate a stack slot (pointer to the argument type) for the argument.
            let alloca_var = LocalVariable::new(
                Dtype::ptr_to(dtype.clone()),
                self.alloc_vreg(),
                Some(id.to_string()),
            );
            self.emit_alloca(Operand::from(alloca_var.clone()));
            // Store the incoming value into the newly allocated stack slot.
            self.emit_store(Operand::from(var), Operand::from(alloca_var.clone()));
            self.local_variables.insert(id.clone(), alloca_var);
        }

        // Lower the function body statement by statement.
        for stmt in from.stmts.iter() {
            self.handle_block(stmt, None, None)?;
        }

        // Append an implicit return if the last instruction is not already a return.
        if let Some(stmt) = self.irs.last() {
            if !matches!(stmt.inner, StmtInner::Return(_)) {
                match &return_dtype {
                    Dtype::I32 => {
                        self.emit_return(Some(Operand::from(0)));
                    }
                    Dtype::Void => {
                        self.emit_return(None);
                    }
                    _ => return Err(Error::ReturnTypeUnsupported),
                }
            }
        }

        Ok(())
    }
}

// ── Statement handlers ────────────────────────────────────────────────────────

impl<'ir> FunctionGenerator<'ir> {
    /// Dispatches a single code-block statement to the appropriate handler.
    ///
    /// `con_label` and `bre_label` are the jump targets for `continue` and
    /// `break` inside the current loop, respectively.  Both are `None` when the
    /// statement is not nested inside a loop.
    pub fn handle_block(
        &mut self,
        stmt: &ast::CodeBlockStmt,
        con_label: Option<BlockLabel>,
        bre_label: Option<BlockLabel>,
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
    /// If the left-hand side resolves to an `Undecided` type (a bare identifier
    /// that has not been declared yet), a new stack slot is allocated with the
    /// type inferred from the right-hand side, and the variable is registered as
    /// a new scoped local.
    pub fn handle_assignment_stmt(&mut self, stmt: &AssignmentStmt) -> Result<(), Error> {
        let mut left = self.handle_left_val(&stmt.left_val)?;
        let right = self.handle_right_val(&stmt.right_val)?;

        // Left side has no concrete type yet — allocate a slot and register as a new local.
        if left.dtype() == &Dtype::Undecided {
            let left_name = match &stmt.left_val.inner {
                ast::LeftValInner::Id(id) => Some(id.clone()),
                _ => None,
            };
            // Infer the stack-slot type from the right-hand-side value.
            let right_type = right.dtype();
            let local_val = LocalVariable::new(
                Dtype::ptr_to(right_type.clone()),
                self.alloc_vreg(),
                left_name.clone(),
            );
            left = Operand::from(local_val.clone());
            self.emit_alloca(left.clone());

            let local_name = left_name.ok_or(Error::SymbolMissing)?;
            let inserted = self.local_variables.insert(local_name.clone(), local_val);
            if inserted.is_none() {
                self.record_scoped_local(local_name);
            }
        }

        // Write the right-hand-side value into the destination slot.
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
        variable: LocalVariable,
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

    /// Creates a new pointer-typed local variable and emits an `alloca` for it.
    ///
    /// Returns the resulting [`LocalVariable`] whose type is `*pointee`.
    fn allocate_pointer_local(&mut self, identifier: &str, pointee: Dtype) -> LocalVariable {
        let variable = LocalVariable::new(
            Dtype::ptr_to(pointee),
            self.alloc_vreg(),
            Some(identifier.to_string()),
        );
        self.emit_alloca(Operand::from(variable.clone()));
        variable
    }

    /// Allocates stack space for a scalar local and initializes it with `right_val`.
    ///
    /// Combines [`allocate_pointer_local`] with an immediate `store` instruction.
    fn define_scalar_local(
        &mut self,
        identifier: &str,
        pointee: Dtype,
        right_val: Operand,
    ) -> LocalVariable {
        let local = self.allocate_pointer_local(identifier, pointee);
        self.emit_store(right_val, Operand::from(local.clone()));
        local
    }

    /// Determines the storage strategy for a variable *declaration* (no initialiser).
    ///
    /// Returns `Deferred` for untyped scalars (type to be resolved at first
    /// assignment), and `Alloca` for typed scalars and all arrays.
    fn plan_local_decl_storage(decl: &ast::VarDecl) -> Result<LocalStoragePlan, Error> {
        let dtype = decl.type_specifier.as_ref().map(Dtype::from);
        match (&decl.inner, dtype.as_ref()) {
            (ast::VarDeclInner::Scalar, None) => Ok(LocalStoragePlan::Deferred),
            (ast::VarDeclInner::Scalar, Some(Dtype::I32)) => {
                Ok(LocalStoragePlan::Alloca(Dtype::I32))
            }
            (ast::VarDeclInner::Scalar, Some(Dtype::Struct { type_name })) => {
                Ok(LocalStoragePlan::Alloca(Dtype::Struct {
                    type_name: type_name.clone(),
                }))
            }
            (ast::VarDeclInner::Array(arr), None | Some(Dtype::I32)) => Ok(
                LocalStoragePlan::Alloca(Dtype::array_of(Dtype::I32, arr.len)),
            ),
            (ast::VarDeclInner::Array(arr), Some(Dtype::Struct { type_name })) => {
                Ok(LocalStoragePlan::Alloca(Dtype::array_of(
                    Dtype::Struct {
                        type_name: type_name.clone(),
                    },
                    arr.len,
                )))
            }
            _ => Err(Error::LocalVarDefinitionUnsupported),
        }
    }

    /// Determines the storage strategy for a scalar variable *definition*.
    ///
    /// Returns `Deferred` when no explicit type annotation is present so that
    /// the type is inferred from the initialiser expression.
    fn plan_local_scalar_def_storage(dtype: &Option<Dtype>) -> Result<LocalStoragePlan, Error> {
        match dtype.as_ref() {
            None => Ok(LocalStoragePlan::Deferred),
            Some(Dtype::I32) => Ok(LocalStoragePlan::Alloca(Dtype::I32)),
            Some(Dtype::Struct { type_name }) => Ok(LocalStoragePlan::Alloca(Dtype::Struct {
                type_name: type_name.clone(),
            })),
            _ => Err(Error::LocalVarDefinitionUnsupported),
        }
    }

    /// Returns the concrete array [`Dtype`] for an array variable definition.
    ///
    /// Only `i32` element arrays are currently supported; other element types
    /// return `LocalVarDefinitionUnsupported`.
    fn plan_local_array_def_storage(dtype: &Option<Dtype>, len: usize) -> Result<Dtype, Error> {
        match dtype.as_ref() {
            None | Some(Dtype::I32) => Ok(Dtype::array_of(Dtype::I32, len)),
            _ => Err(Error::LocalVarDefinitionUnsupported),
        }
    }

    /// Lowers a local variable declaration (without an initialiser).
    ///
    /// Allocates storage according to [`plan_local_decl_storage`] and registers
    /// the variable in the current scope.
    pub fn handle_local_var_decl(&mut self, decl: &ast::VarDecl) -> Result<(), Error> {
        let identifier = decl.identifier.as_str();
        let variable = match Self::plan_local_decl_storage(decl)? {
            LocalStoragePlan::Deferred => LocalVariable::new(
                Dtype::Undecided,
                self.alloc_vreg(),
                Some(identifier.to_string()),
            ),
            LocalStoragePlan::Alloca(pointee) => self.allocate_pointer_local(identifier, pointee),
        };
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
    pub fn init_array(&mut self, base_ptr: Operand, vals: &RightValList) -> Result<(), Error> {
        for (i, val) in vals.iter().enumerate() {
            let element_ptr = self.alloc_temporary(Dtype::ptr_to(Dtype::I32));
            let right_elem = self.handle_right_val(val)?;

            self.emit_gep(
                element_ptr.clone(),
                base_ptr.clone(),
                Operand::from(i as i32),
            );
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
        base_ptr: Operand,
        initializer: &ArrayInitializer,
    ) -> Result<(), Error> {
        match initializer {
            ArrayInitializer::ExplicitList(vals) => self.init_array(base_ptr, vals),
            ArrayInitializer::Fill { val, count } => {
                let fill_val = self.handle_right_val(val)?;
                for i in 0..*count {
                    let element_ptr = self.alloc_temporary(Dtype::ptr_to(Dtype::I32));
                    self.emit_gep(
                        element_ptr.clone(),
                        base_ptr.clone(),
                        Operand::from(i as i32),
                    );
                    self.emit_store(fill_val.clone(), element_ptr);
                }
                Ok(())
            }
        }
    }

    /// Lowers a local variable definition (declaration with an initializer).
    ///
    /// Handles both scalar and array initializers, then registers the resulting
    /// local variable in the current scope.
    pub fn handle_local_var_def(&mut self, def: &ast::VarDef) -> Result<(), Error> {
        let identifier = def.identifier.as_str();
        let dtype = def.type_specifier.as_ref().map(Dtype::from);

        let variable: LocalVariable = match &def.inner {
            ast::VarDefInner::Scalar(scalar) => {
                let right_val = self.handle_right_val(&scalar.val)?;
                match Self::plan_local_scalar_def_storage(&dtype)? {
                    LocalStoragePlan::Deferred => {
                        self.define_scalar_local(identifier, right_val.dtype().clone(), right_val)
                    }
                    LocalStoragePlan::Alloca(pointee) => {
                        self.define_scalar_local(identifier, pointee, right_val)
                    }
                }
            }
            ast::VarDefInner::Array(array) => {
                let pointee = Self::plan_local_array_def_storage(&dtype, array.len)?;
                let local = self.allocate_pointer_local(identifier, pointee);
                self.init_array_from(Operand::from(local.clone()), &array.initializer)?;
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
        for arg in stmt.fn_call.vals.iter() {
            let right_val = self.handle_right_val(arg)?;
            args.push(right_val);
        }

        match self.registry.function_types.get(&function_name) {
            None => Err(Error::FunctionNotDefined {
                symbol: function_name,
            }),
            Some(function_type) => {
                let retval = match &function_type.return_dtype {
                    Dtype::Void => Ok(None),
                    Dtype::I32 | Dtype::Struct { .. } => Ok(Some(
                        self.alloc_temporary(function_type.return_dtype.clone()),
                    )),
                    _ => Err(Error::FunctionCallUnsupported),
                }?;
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
        con_label: Option<BlockLabel>,
        bre_label: Option<BlockLabel>,
    ) -> Result<(), Error> {
        let true_label = self.alloc_basic_block();
        let false_label = self.alloc_basic_block();
        let after_label = self.alloc_basic_block();

        // Evaluate the condition; jump to the appropriate branch.
        self.handle_bool_unit(&stmt.bool_unit, true_label.clone(), false_label.clone())?;

        // Emit the then-branch; a new scope is opened so that any locals are cleaned up.
        self.emit_label(true_label);
        self.enter_scope();
        for s in stmt.if_stmts.iter() {
            self.handle_block(s, con_label.clone(), bre_label.clone())?;
        }
        self.exit_scope();
        // Jump past the else-branch to the merge point.
        self.emit_jump(after_label.clone());

        // Emit the (possibly absent) else-branch in its own scope.
        self.emit_label(false_label);
        self.enter_scope();
        if let Some(else_stmts) = &stmt.else_stmts {
            for s in else_stmts.iter() {
                self.handle_block(s, con_label.clone(), bre_label.clone())?;
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
        for s in stmt.stmts.iter() {
            self.handle_block(s, Some(test_label.clone()), Some(false_label.clone()))?;
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
    pub fn handle_continue_stmt(&mut self, con_label: Option<BlockLabel>) -> Result<(), Error> {
        let label = con_label.ok_or(Error::InvalidContinueInst)?;
        self.emit_jump(label);
        Ok(())
    }

    /// Lowers a `break` statement by jumping to the enclosing loop's exit label.
    ///
    /// Returns `InvalidBreakInst` if called outside of a loop context.
    pub fn handle_break_stmt(&mut self, bre_label: Option<BlockLabel>) -> Result<(), Error> {
        let label = bre_label.ok_or(Error::InvalidBreakInst)?;
        self.emit_jump(label);
        Ok(())
    }
}

// ── Expression and value handlers ─────────────────────────────────────────────

impl<'ir> FunctionGenerator<'ir> {
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

        let dst = self.alloc_temporary(Dtype::I1);
        self.emit_cmp(
            CmpPredicate::from(expr.op.clone()),
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

                let res = match &return_dtype {
                    Dtype::I32 | Dtype::Struct { .. } => self.alloc_temporary(return_dtype.clone()),
                    _ => {
                        return Err(Error::InvalidExprUnit {
                            expr_unit: unit.clone(),
                        });
                    }
                };

                let mut args: Vec<Operand> = Vec::new();
                for arg in fn_call.vals.iter() {
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
                let dst = self.alloc_temporary(pointee.as_ref().clone());
                self.emit_load(dst.clone(), operand);
                dst
            }
            // Auto-load global i32 values which are stored behind a pointer.
            Dtype::I32 if matches!(&operand, Operand::Global(_)) => {
                let dst = self.alloc_temporary(Dtype::I32);
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
        let target = self.alloc_temporary(Dtype::ptr_to(Dtype::Array {
            element: Box::new(element_type),
            length: None,
        }));
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
                let loaded = self.alloc_temporary(pointee.as_ref().clone());
                self.emit_load(loaded.clone(), arr);
                (loaded.clone(), loaded.dtype().clone())
            }
            _ => (arr.clone(), arr.dtype().clone()),
        };

        let target = match &arr_dtype {
            Dtype::Pointer { pointee } => match pointee.as_ref() {
                Dtype::Array { element, .. } => {
                    Ok(self.alloc_temporary(Dtype::ptr_to(element.as_ref().clone())))
                }
                _ => Ok(self.alloc_temporary(Dtype::ptr_to(pointee.as_ref().clone()))),
            },
            Dtype::Array { element, .. } => {
                Ok(self.alloc_temporary(Dtype::ptr_to(element.as_ref().clone())))
            }
            _ => Err(Error::InvalidArrayExpression),
        }?;

        let index = self.handle_index_expr(expr.idx.as_ref())?;
        self.emit_gep(target.clone(), arr, index);

        Ok(target)
    }

    /// Lowers a struct member access expression (`s.member`) to a member pointer.
    ///
    /// Looks up the struct type in the registry, finds the member's byte offset,
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
        let member_offset = member.offset;

        let target = match &member_dtype {
            Dtype::Void | Dtype::Undecided => {
                return Err(Error::InvalidStructMemberExpression { expr: expr.clone() })
            }
            _ => self.alloc_temporary(Dtype::ptr_to(member_dtype)),
        };

        self.emit_gep(target.clone(), s, Operand::from(member_offset));
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
        let dst = self.alloc_temporary(Dtype::I32);
        self.emit_biop(ArithBinOp::from(expr.op.clone()), left, right, dst.clone());
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
                let idx = self.alloc_temporary(Dtype::I32);
                self.emit_load(idx.clone(), src);
                Ok(idx)
            }
            ast::IndexExprInner::Num(num) => Ok(Operand::from(*num as i32)),
        }
    }
}

// ── Boolean expression handlers ───────────────────────────────────────────────

impl<'ir> FunctionGenerator<'ir> {
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
        let bool_evaluated = self.alloc_temporary(Dtype::ptr_to(Dtype::I32));
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
        let loaded = self.alloc_temporary(Dtype::I32);
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
