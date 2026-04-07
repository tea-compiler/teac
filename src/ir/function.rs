//! IR-level function representation and code generation for the `teac` compiler.
//!
//! This module defines the data structures used to represent functions in the
//! intermediate representation (IR), as well as [`FunctionGenerator`], a stateful
//! builder that translates source-level constructs into a flat list of IR statements
//! similar to LLVM IR. Each function is eventually assembled from basic blocks, each
//! of which contains a sequence of [`Stmt`] instructions.

use super::error::Error;
use super::module::Registry;
use super::stmt::{ArithBinOp, CmpPredicate, Stmt};
use super::types::Dtype;
use super::value::{GlobalVariable, LocalVariable, Operand};
use indexmap::IndexMap;
use std::fmt::{Display, Formatter};

/// A label that identifies either a numbered basic block or a named function entry point.
///
/// Labels are used as branch targets in control-flow instructions and as keys
/// when building a basic-block map during IR lowering.
#[derive(Clone)]
pub enum BlockLabel {
    /// A numbered basic block label, displayed as `bb0`, `bb1`, etc.
    BasicBlock(usize),
    /// A named function entry label, displayed as the function's identifier string.
    Function(String),
}

impl Display for BlockLabel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockLabel::BasicBlock(index) => write!(f, "bb{}", index),
            BlockLabel::Function(identifier) => write!(f, "{}", identifier),
        }
    }
}

impl BlockLabel {
    /// Returns the string representation of this label, used as a map key.
    pub fn key(&self) -> String {
        format!("{}", self)
    }
}

/// A basic block: an ordered sequence of IR statements preceded by a label.
///
/// In well-formed IR each basic block ends with a terminator instruction
/// (jump, conditional jump, or return).
#[derive(Clone)]
pub struct BasicBlock {
    /// The label that identifies this basic block and serves as a branch target.
    pub label: BlockLabel,
    /// The ordered list of IR statements that form the body of this basic block.
    pub stmts: Vec<Stmt>,
}

/// The final IR representation of a compiled function.
///
/// This struct is produced after code generation is complete and holds all
/// information needed for subsequent optimization and assembly-emission passes.
pub struct Function {
    /// The source-level name of the function.
    pub identifier: String,
    /// All local variables declared in the function, keyed by their source-level
    /// name. `None` before code generation has finished populating the map.
    pub local_variables: Option<IndexMap<String, LocalVariable>>,
    /// The basic blocks that make up the function body, in emission order.
    /// `None` before the flat IR statement list has been split into blocks.
    pub blocks: Option<Vec<BasicBlock>>,
    /// The function's formal parameters as a list of local variables.
    pub arguments: Vec<LocalVariable>,
    /// The next available virtual register index; preserved so that further
    /// passes can allocate new temporaries without colliding with existing ones.
    pub next_vreg: usize,
}

/// Stateful builder used during IR generation to translate source-level constructs
/// into a flat list of IR statements for a single function.
///
/// After all statements have been emitted the caller splits the flat `irs` list
/// into [`BasicBlock`]s and wraps them together with the remaining fields into a
/// [`Function`].
pub struct FunctionGenerator<'ir> {
    /// Shared type registry containing struct and function type definitions for
    /// the whole module.
    pub registry: &'ir Registry,
    /// Reference to the module's global variable map, used during variable lookup.
    pub global_variables: &'ir IndexMap<String, GlobalVariable>,
    /// Map of currently visible local variables, keyed by their source-level name.
    /// Variables are inserted when declared and removed when their enclosing scope
    /// exits.
    pub local_variables: IndexMap<String, LocalVariable>,
    /// A stack of scopes. Each entry is the list of local variable names introduced
    /// in that scope, enabling bulk removal when the scope exits via [`exit_scope`].
    ///
    /// [`exit_scope`]: FunctionGenerator::exit_scope
    scope_locals: Vec<Vec<String>>,
    /// The flat list of IR statements being accumulated for the current function.
    pub irs: Vec<Stmt>,
    /// The function's formal parameters as local variables.
    pub arguments: Vec<LocalVariable>,
    /// Counter for allocating unique virtual register indices; incremented by
    /// [`alloc_vreg`].
    ///
    /// [`alloc_vreg`]: FunctionGenerator::alloc_vreg
    pub next_vreg: usize,
    /// Counter for allocating unique basic block label indices; starts at `1`
    /// because index `0` is reserved for the implicit function-entry block.
    pub next_basic_block: usize,
}

impl<'ir> FunctionGenerator<'ir> {
    /// Constructs a new [`FunctionGenerator`] with empty state, ready to build a
    /// function body. Virtual register allocation starts at `0` and basic block
    /// label allocation starts at `1`.
    pub fn new(
        registry: &'ir Registry,
        global_variables: &'ir IndexMap<String, GlobalVariable>,
    ) -> Self {
        Self {
            registry,
            global_variables,
            local_variables: IndexMap::new(),
            scope_locals: Vec::new(),
            irs: Vec::new(),
            arguments: Vec::new(),
            next_vreg: 0,
            next_basic_block: 1,
        }
    }

    /// Allocates and returns the next unique virtual register index, then advances
    /// the internal counter.
    pub fn alloc_vreg(&mut self) -> usize {
        let idx = self.next_vreg;
        self.next_vreg += 1;
        idx
    }

    /// Creates an unnamed temporary [`Operand`] of the given data type, backed by a
    /// freshly allocated virtual register.
    pub fn alloc_temporary(&mut self, dtype: Dtype) -> Operand {
        Operand::from(LocalVariable::new(dtype, self.alloc_vreg(), None))
    }

    /// Allocates and returns a new unique [`BlockLabel::BasicBlock`] label, then
    /// advances the internal counter.
    pub fn alloc_basic_block(&mut self) -> BlockLabel {
        let idx = self.next_basic_block;
        self.next_basic_block += 1;
        BlockLabel::BasicBlock(idx)
    }

    /// Resolves a variable name to an [`Operand`].
    ///
    /// Lookup order:
    /// 1. Local variables in [`local_variables`] (innermost scope wins).
    /// 2. Global variables in [`global_variables`].
    ///
    /// Returns [`Error::VariableNotDefined`] if the name is not found in either map.
    ///
    /// [`local_variables`]: FunctionGenerator::local_variables
    /// [`global_variables`]: FunctionGenerator::global_variables
    pub fn lookup_variable(&self, id: &str) -> Result<Operand, Error> {
        if let Some(local) = self.local_variables.get(id) {
            Ok(Operand::from(local))
        } else if let Some(global) = self.global_variables.get(id) {
            Ok(Operand::Global(global.clone()))
        } else {
            Err(Error::VariableNotDefined {
                symbol: id.to_string(),
            })
        }
    }

    /// Pushes a new empty lexical scope onto the scope stack.
    ///
    /// Call this before entering any block (e.g., `{` in the source language) so
    /// that variables declared inside can be tracked and later removed by
    /// [`exit_scope`].
    ///
    /// [`exit_scope`]: FunctionGenerator::exit_scope
    pub fn enter_scope(&mut self) {
        self.scope_locals.push(Vec::new());
    }

    /// Pops the innermost lexical scope from the scope stack and removes all local
    /// variables that were introduced in that scope from [`local_variables`].
    ///
    /// [`local_variables`]: FunctionGenerator::local_variables
    pub fn exit_scope(&mut self) {
        if let Some(locals) = self.scope_locals.pop() {
            for id in locals {
                self.local_variables.shift_remove(&id);
            }
        }
    }

    /// Registers a local variable name in the current (innermost) scope so that it
    /// will be removed from [`local_variables`] when [`exit_scope`] is called.
    ///
    /// [`local_variables`]: FunctionGenerator::local_variables
    /// [`exit_scope`]: FunctionGenerator::exit_scope
    pub fn record_scoped_local(&mut self, id: String) {
        if let Some(scope) = self.scope_locals.last_mut() {
            scope.push(id);
        }
    }
}

impl FunctionGenerator<'_> {
    /// Emits a stack-allocation (`alloca`) instruction that reserves space for `dst`.
    pub fn emit_alloca(&mut self, dst: Operand) {
        self.irs.push(Stmt::as_alloca(dst));
    }

    /// Emits a memory-load instruction that reads a value from `ptr` into `dst`.
    pub fn emit_load(&mut self, dst: Operand, ptr: Operand) {
        self.irs.push(Stmt::as_load(dst, ptr));
    }

    /// Emits a memory-store instruction that writes `src` to the address `ptr`.
    pub fn emit_store(&mut self, src: Operand, ptr: Operand) {
        self.irs.push(Stmt::as_store(src, ptr));
    }

    /// Emits a get-element-pointer (GEP) instruction that computes the address of
    /// `base_ptr[index]` and stores it in `new_ptr`.
    pub fn emit_gep(&mut self, new_ptr: Operand, base_ptr: Operand, index: Operand) {
        self.irs.push(Stmt::as_gep(new_ptr, base_ptr, index));
    }

    /// Emits an arithmetic binary operation (`op`) on `left` and `right`, storing
    /// the result in `dst`.
    pub fn emit_biop(&mut self, op: ArithBinOp, left: Operand, right: Operand, dst: Operand) {
        self.irs.push(Stmt::as_biop(op, left, right, dst));
    }

    /// Emits an integer comparison instruction using predicate `op` on `left` and
    /// `right`, storing the boolean result in `dst`.
    pub fn emit_cmp(&mut self, op: CmpPredicate, left: Operand, right: Operand, dst: Operand) {
        self.irs.push(Stmt::as_cmp(op, left, right, dst));
    }

    /// Emits a conditional branch instruction that jumps to `true_label` when `cond`
    /// is non-zero and to `false_label` otherwise.
    pub fn emit_cjump(&mut self, cond: Operand, true_label: BlockLabel, false_label: BlockLabel) {
        self.irs.push(Stmt::as_cjump(cond, true_label, false_label));
    }

    /// Emits an unconditional branch instruction that transfers control to `target`.
    pub fn emit_jump(&mut self, target: BlockLabel) {
        self.irs.push(Stmt::as_jump(target));
    }

    /// Emits a basic block label marker, signalling the start of a new basic block
    /// identified by `label`.
    pub fn emit_label(&mut self, label: BlockLabel) {
        self.irs.push(Stmt::as_label(label));
    }

    /// Emits a function-call instruction that invokes `func_name` with `args`,
    /// optionally storing the return value in `result`.
    pub fn emit_call(&mut self, func_name: String, result: Option<Operand>, args: Vec<Operand>) {
        self.irs.push(Stmt::as_call(func_name, result, args));
    }

    /// Emits a return instruction, optionally carrying a return value `val`.
    pub fn emit_return(&mut self, val: Option<Operand>) {
        self.irs.push(Stmt::as_return(val));
    }
}
