//! IR generation from the AST at the module (translation-unit) level.
//!
//! This module implements the [`Generator`] trait for [`IrGenerator`], driving
//! the full compilation pipeline from a parsed AST program to a populated IR
//! module ready for emission.  The implementation also contains a collection
//! of private helper methods on `IrGenerator` that handle each category of
//! top-level AST node (use statements, global variable declarations, function
//! declarations, function definitions, and struct definitions).

use crate::ast;
use crate::ir::function::{
    BasicBlock, BlockLabel, Function, FunctionBody, FunctionGenerator,
};
use crate::ir::gen::type_infer;
use crate::ir::module::IrGenerator;
use crate::ir::printer::IrPrinter;
use crate::ir::stmt::{Stmt, StmtInner};
use crate::ir::types::{Dtype, FunctionType, StructMember, StructType};
use crate::ir::value::GlobalDef;
use crate::ir::Error;

use crate::common::Generator;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::rc::Rc;

/// Implements the two-phase `Generator` trait for the module-level IR generator.
impl Generator for IrGenerator<'_> {
    type Error = Error;

    /// Drive the three-pass IR generation pipeline for the whole program:
    ///
    /// 1. **Use-statement pass** — process every `use` statement so that
    ///    external symbols (e.g. the standard library) are pre-registered
    ///    before any declarations reference them.
    /// 2. **Declaration/definition registration pass** — iterate over all
    ///    top-level elements and register global variables, function
    ///    declarations, function definitions (signature only), and struct
    ///    definitions into the module and type registry.
    /// 3. **Function body generation pass** — for each function definition,
    ///    invoke the `FunctionGenerator` to emit flat IR statements, then
    ///    convert those statements into structured basic blocks via
    ///    `harvest_function_irs`, and store the result back into the module.
    fn generate(&mut self) -> Result<(), Error> {
        let input = self.input;

        // Pass 1: handle `use` statements so imported symbols are available.
        for use_stmt in &input.use_stmts {
            self.handle_use_stmt(use_stmt)?;
        }

        // Pass 2: register all declarations and definitions (signatures only).
        for elem in &input.elements {
            match &elem.inner {
                ast::ProgramElementInner::VarDeclStmt(stmt) => {
                    self.handle_global_var_decl(stmt)?;
                }
                ast::ProgramElementInner::FnDeclStmt(fn_decl) => self.handle_fn_decl(fn_decl)?,
                ast::ProgramElementInner::FnDef(fn_def) => self.handle_fn_def(fn_def)?,
                ast::ProgramElementInner::StructDef(struct_def) => {
                    self.handle_struct_def(struct_def)?;
                }
            }
        }

        // Pass 3: generate IR bodies for every function definition.
        for elem in &input.elements {
            if let ast::ProgramElementInner::FnDef(fn_def) = &elem.inner {
                // Run the type inference pass to resolve all local variable
                // types before IR generation.  The pass sees the same name
                // environment as `FunctionGenerator` — struct/function types
                // from the registry plus the module's global variable list —
                // so every identifier inside the function body resolves
                // consistently in both passes.
                let resolved_types = type_infer::infer_function(
                    &self.registry,
                    &self.module.global_list,
                    fn_def,
                )?;

                // Use a scoped FunctionGenerator so its temporary state is
                // dropped before we mutably borrow `self.module` below.
                let body = {
                    let mut function_generator = FunctionGenerator::new(
                        &self.registry,
                        &self.module.global_list,
                        resolved_types,
                    );
                    function_generator.generate(fn_def)?;

                    FunctionBody {
                        arguments: function_generator.arguments,
                        blocks: Self::harvest_function_irs(function_generator.irs),
                        next_vreg: function_generator.next_vreg,
                    }
                };

                // Attach the body to the Function entry created during pass 2.
                match self
                    .module
                    .function_list
                    .get_mut(&fn_def.fn_decl.identifier)
                {
                    Some(f) => f.body = Some(body),
                    None => {
                        return Err(Error::FunctionNotDefined {
                            symbol: fn_def.fn_decl.identifier.clone(),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Emit the complete IR module to the provided writer in textual form.
    ///
    /// The output is structured as follows:
    /// 1. **Header** — target triple and data-layout string.
    /// 2. **Struct type definitions** — one line per registered struct type.
    /// 3. **Global variables** — all global variable declarations/definitions.
    /// 4. **Functions** — for each function, either a full definition (if a
    ///    body is present) or an external declaration (if body is absent).
    fn output<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let mut printer = IrPrinter::new(w);

        // Emit the LLVM-style target triple and data layout header.
        printer.emit_header(Self::TARGET_TRIPLE, Self::TARGET_DATALAYOUT)?;

        // Emit all struct type definitions collected during IR generation.
        for (name, st) in &self.registry.struct_types {
            printer.emit_struct_type(name, st)?;
        }
        printer.emit_newline()?;

        // Emit all global variable declarations and definitions.
        for (name, def) in &self.module.global_list {
            printer.emit_global(name, def)?;
        }
        printer.emit_newline()?;

        // Emit each function — as a definition if it has a body, or as an
        // external declaration otherwise.
        for func in self.module.function_list.values() {
            let func_type = self
                .registry
                .function_types
                .get(&func.identifier)
                .ok_or_else(|| Error::FunctionNotDefined {
                    symbol: func.identifier.clone(),
                })?;
            match &func.body {
                Some(body) => {
                    printer.emit_function_def(&func.identifier, &func_type.return_dtype, body)?;
                }
                None => {
                    printer.emit_function_decl(&func.identifier, func_type)?;
                }
            }
        }

        Ok(())
    }
}

/// Private helper methods on `IrGenerator` for each category of top-level AST node.
impl IrGenerator<'_> {
    /// Process a single `use` statement from the source program.
    ///
    /// Resolves the path to `<module_name>.teah` relative to the source
    /// file's directory (`self.source_dir`), reads and parses it using
    /// the existing [`crate::parser::Parser`], and registers every
    /// `fn` declaration found in that header into the type registry
    /// and module function list.
    ///
    /// The function name stored in the registry is qualified with the
    /// module prefix — e.g. a declaration `fn getint() -> i32;` in
    /// `std.teah` is registered as `"std::getint"` — to match the
    /// `std::getint()` call-site syntax used in TeaLang source files.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModuleNotFound`] if the `.teah` file does not
    /// exist, [`Error::ModuleParseFailed`] if it cannot be parsed, and
    /// propagates any [`Error::Io`] encountered while reading the file.
    fn handle_use_stmt(&mut self, use_stmt: &ast::UseStmt) -> Result<(), Error> {
        let module_name = &use_stmt.module_name;
        let header_path = self.source_dir.join(format!("{module_name}.teah"));

        if !header_path.exists() {
            return Err(Error::ModuleNotFound {
                module_name: module_name.clone(),
                path: header_path,
            });
        }

        let source = fs::read_to_string(&header_path)?;
        let mut parser = crate::parser::Parser::new(&source);
        parser.generate().map_err(|e| Error::ModuleParseFailed {
            module_name: module_name.clone(),
            message: e.to_string(),
        })?;

        if let Some(program) = parser.program {
            for elem in &program.elements {
                // Header files are expected to contain only `fn` declarations.
                // Other element kinds (global variables, struct definitions,
                // function bodies) are not valid in a `.teah` file and are
                // silently skipped.
                if let ast::ProgramElementInner::FnDeclStmt(fn_decl_stmt) = &elem.inner {
                    // Qualify the function name with the module prefix so that
                    // call sites such as `std::getint()` resolve correctly.
                    let mut prefixed_decl = fn_decl_stmt.fn_decl.as_ref().clone();
                    prefixed_decl.identifier =
                        format!("{module_name}::{}", prefixed_decl.identifier);
                    self.handle_fn_decl(&prefixed_decl)?;
                }
            }
        }

        Ok(())
    }

    /// Convert a flat list of IR statements produced by `FunctionGenerator`
    /// into a list of [`BasicBlock`]s.
    ///
    /// ## Basic-block construction
    ///
    /// The flat statement list uses `Label` pseudo-instructions as block
    /// boundaries.  This function walks the list and starts a new basic block
    /// each time it encounters a `Label`.  When the *next* label is seen (or
    /// the end of the list is reached) the accumulated statements are flushed
    /// into the current block.
    ///
    /// Statements that appear before the first label, or after a terminator
    /// (`Return`, `CJump`, or `Jump`) within the same block, are **dead code**
    /// and are silently dropped.  The `terminated` flag tracks whether the
    /// current block has already received a terminator so that subsequent
    /// statements can be skipped until the next label is seen.
    ///
    /// ## Alloca hoisting
    ///
    /// After all blocks are formed, any `Alloca` instruction found in a
    /// non-entry block is moved to the *beginning* of the entry block (block
    /// index 0).  This matches the LLVM convention that all stack allocations
    /// should appear in the function's entry block, which simplifies later
    /// analyses and code generation.
    ///
    /// ## Empty-block cleanup
    ///
    /// Once allocas have been hoisted some non-entry blocks may be left with
    /// no statements (e.g. a block that contained *only* allocas).  Such
    /// blocks are removed because an empty basic block (one with a label but
    /// no terminator) is invalid IR.
    fn harvest_function_irs(irs: Vec<Stmt>) -> Vec<BasicBlock> {
        let mut blocks = Vec::new();
        let mut label: Option<BlockLabel> = None;
        let mut stmts = Vec::new();
        let mut terminated = false;

        for stmt in irs {
            if let StmtInner::Label(l) = &stmt.inner {
                // Finalise the previous block (if any) and start a new one.
                if let Some(prev_label) = label.take() {
                    blocks.push(BasicBlock {
                        label: prev_label,
                        stmts: std::mem::take(&mut stmts),
                    });
                }
                label = Some(l.label.clone());
                terminated = false;
            } else {
                // Drop statements that precede the first label or follow a
                // terminator — they are unreachable (dead) code.
                if label.is_none() || terminated {
                    continue;
                }
                terminated = matches!(
                    &stmt.inner,
                    StmtInner::Return(_) | StmtInner::CJump(_) | StmtInner::Jump(_)
                );
                stmts.push(stmt);
            }
        }
        if let Some(last_label) = label {
            blocks.push(BasicBlock {
                label: last_label,
                stmts,
            });
        }

        if blocks.is_empty() {
            return blocks;
        }

        // Hoist all allocas from non-entry blocks to the entry block, right
        // after the entry label.  This ensures all stack allocations happen in
        // the entry block (LLVM convention).
        let mut hoisted_allocas: Vec<Stmt> = Vec::new();
        for block in blocks.iter_mut().skip(1) {
            let (allocas, remaining): (Vec<Stmt>, Vec<Stmt>) = block
                .stmts
                .drain(..)
                .partition(|x| matches!(&x.inner, StmtInner::Alloca(_)));
            hoisted_allocas.extend(allocas);
            block.stmts = remaining;
        }
        // Insert hoisted allocas at the beginning of the entry block.
        blocks[0].stmts.splice(0..0, hoisted_allocas);

        // Post-hoist invariant: every block still has at least a terminator
        // (`return` / `jump` / `cjump`) because the IR generator always emits
        // one for reachable blocks, and the terminator is not an alloca so it
        // is never hoisted away.  Blocks that somehow ended up empty (only an
        // alloca-only body) would become dangling jump targets if dropped, so
        // we verify — and drop — them together with the edges that reach
        // them.
        Self::drop_empty_blocks_or_panic(&mut blocks);

        blocks
    }

    /// Removes empty basic blocks (no instructions after alloca hoisting) and
    /// panics if any of them are still referenced by a `jump`/`cjump` in a
    /// surviving block, since that would leave dangling branch targets.
    ///
    /// Under normal code-generator behaviour no block ever becomes empty, so
    /// this routine is almost always a no-op; the panic fires only if the
    /// generator breaks the "every reachable block has a terminator"
    /// invariant.
    fn drop_empty_blocks_or_panic(blocks: &mut Vec<BasicBlock>) {
        if blocks.iter().all(|b| !b.stmts.is_empty()) {
            return;
        }

        // Collect the labels referenced by any remaining (non-empty) block's
        // terminator so we can tell whether an empty block is still reachable.
        let mut referenced: HashSet<String> = HashSet::new();
        for block in blocks.iter().filter(|b| !b.stmts.is_empty()) {
            for stmt in &block.stmts {
                match &stmt.inner {
                    StmtInner::Jump(j) => {
                        referenced.insert(j.target.key());
                    }
                    StmtInner::CJump(c) => {
                        referenced.insert(c.true_label.key());
                        referenced.insert(c.false_label.key());
                    }
                    _ => {}
                }
            }
        }

        for block in blocks.iter().filter(|b| b.stmts.is_empty()) {
            let key = block.label.key();
            assert!(
                !referenced.contains(&key),
                "BUG: basic block {key} is empty after alloca hoisting but \
                 is still the target of a jump; IR generator produced a \
                 reachable block without a terminator",
            );
        }

        blocks.retain(|block| !block.stmts.is_empty());
    }

    /// Process a global variable declaration or definition.
    ///
    /// * Extracts the identifier and resolves the data type from the AST node.
    /// * If the node is a *definition* (not just a declaration), the
    ///   initializer list is evaluated as a vector of static (compile-time
    ///   constant) values:
    ///   - **Array — explicit list**: each element value is evaluated
    ///     individually via `handle_right_val_static`.
    ///   - **Array — fill**: a single value is repeated `count` times.
    ///   - **Scalar**: a single-element vector wrapping the scalar value.
    /// * A [`GlobalDef`] is inserted into the module's global list, keyed by
    ///   its fully-qualified name.  If an entry with the same identifier
    ///   already exists the function returns a [`Error::VariableRedefinition`]
    ///   error.
    fn handle_global_var_decl(&mut self, stmt: &ast::VarDeclStmt) -> Result<(), Error> {
        let identifier = match &stmt.inner {
            ast::VarDeclStmtInner::Decl(d) => d.identifier.clone(),
            ast::VarDeclStmtInner::Def(d) => d.identifier.clone(),
        };

        let dtype = Dtype::try_from(stmt)?;
        let initializers = if let ast::VarDeclStmtInner::Def(d) = &stmt.inner {
            Some(match &d.inner {
                ast::VarDefInner::Array(def) => match &def.initializer {
                    ast::ArrayInitializer::ExplicitList(vals) => vals
                        .iter()
                        .map(Self::handle_right_val_static)
                        .collect::<Result<Vec<_>, _>>()?,
                    ast::ArrayInitializer::Fill { val, count } => {
                        let v = Self::handle_right_val_static(val)?;
                        vec![v; *count]
                    }
                },
                ast::VarDefInner::Scalar(scalar) => {
                    let value = Self::handle_right_val_static(&scalar.val)?;
                    vec![value]
                }
            })
        } else {
            None
        };

        if self.module.global_list.contains_key(identifier.as_str()) {
            return Err(Error::VariableRedefinition { symbol: identifier });
        }
        self.module
            .global_list
            .insert(Rc::from(identifier), GlobalDef { dtype, initializers });
        Ok(())
    }

    /// Process a function declaration (`fn foo(...) -> T;`).
    ///
    /// Steps:
    /// 1. Collect each parameter's name and data type.  Array parameters are
    ///    rejected outright (`Error::ArrayParameterNotAllowed`).
    /// 2. Build a [`FunctionType`] from the parameter list and the optional
    ///    return type (defaults to `void` if absent).
    /// 3. Insert the function type into the registry.  If a type with the
    ///    same identifier already exists and *differs* from the new one, a
    ///    [`Error::ConflictedFunction`] error is returned.  Identical
    ///    re-declarations are silently accepted.
    /// 4. Insert a skeleton [`Function`] (body-less) into the module's
    ///    function list so that the printer can emit an external declaration.
    fn handle_fn_decl(&mut self, decl: &ast::FnDecl) -> Result<(), Error> {
        let identifier = decl.identifier.clone();
        let function_type = FunctionType::try_from(decl)?;

        if let Some(prior) = self
            .registry
            .function_types
            .insert(identifier.clone(), function_type.clone())
        {
            if prior != function_type {
                return Err(Error::ConflictedFunction { symbol: identifier });
            }
        }

        self.module.function_list.insert(
            identifier.clone(),
            Function {
                identifier,
                body: None,
            },
        );

        Ok(())
    }

    /// Process a function definition (`fn foo(...) -> T { ... }`).
    ///
    /// This pass only handles the *signature*; the body is generated later in
    /// `generate`'s third pass.
    ///
    /// * If no prior declaration exists for this function, delegate to
    ///   `handle_fn_decl` to register the signature.
    /// * If a prior declaration already exists, verify that the definition's
    ///   signature matches it exactly; a mismatch yields
    ///   [`Error::DeclDefMismatch`].
    fn handle_fn_def(&mut self, stmt: &ast::FnDef) -> Result<(), Error> {
        let identifier = stmt.fn_decl.identifier.clone();

        match self.registry.function_types.get(&identifier) {
            None => self.handle_fn_decl(&stmt.fn_decl)?,
            Some(prior) => {
                let def_type = FunctionType::try_from(stmt.fn_decl.as_ref())?;
                if *prior != def_type {
                    return Err(Error::DeclDefMismatch {
                        symbol: identifier.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Process a struct type definition.
    ///
    /// Iterates over the struct's member declarations in order, resolving each
    /// member's base type and computing its layout offset (zero-based index
    /// within the struct).
    ///
    /// * If a member's type is itself a struct, the referenced struct type
    ///   must already be registered in the type registry; otherwise an
    ///   [`Error::UndefinedStructMemberType`] error is returned.  This
    ///   enforces forward-declaration ordering for nested struct types.
    /// * Array members are expanded to a `Dtype::Array` wrapping the base
    ///   element type and the declared length.
    /// * The completed [`StructType`] is inserted into the registry under the
    ///   struct's identifier.
    fn handle_struct_def(&mut self, struct_def: &ast::StructDef) -> Result<(), Error> {
        let identifier = struct_def.identifier.clone();
        let mut elements = Vec::new();

        for (index, decl) in struct_def.decls.iter().enumerate() {
            let base_dtype = match decl.type_specifier.as_ref() {
                Some(type_specifier) => type_specifier.into(),
                None => Dtype::Void,
            };

            if let Dtype::Struct { type_name } = &base_dtype {
                if !self.registry.struct_types.contains_key(type_name) {
                    return Err(Error::UndefinedStructMemberType {
                        struct_name: identifier.clone(),
                        member_type: type_name.clone(),
                    });
                }
            }

            elements.push((
                decl.identifier.clone(),
                StructMember {
                    index,
                    dtype: match &decl.inner {
                        ast::VarDeclInner::Scalar => base_dtype,
                        ast::VarDeclInner::Array(array) => Dtype::array_of(base_dtype, array.len),
                    },
                },
            ));
        }

        self.registry
            .struct_types
            .insert(identifier, StructType { elements });

        Ok(())
    }
}
