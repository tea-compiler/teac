use super::cfg::Cfg;
use super::dominator::DominatorInfo;
use super::FunctionPass;
use crate::common::graph::BackwardLiveness;
use crate::ir::function::{BasicBlock, BlockLabel, Function};
use crate::ir::stmt::{OperandRole, Stmt, StmtInner};
use crate::ir::types::Dtype;
use crate::ir::{Local, LocalId, Operand};
use std::collections::{HashMap, HashSet, VecDeque};

pub struct Mem2RegPass;

impl FunctionPass for Mem2RegPass {
    fn run(&self, func: &mut Function) {
        let Some(body) = func.body.as_mut() else {
            return;
        };
        if body.blocks.is_empty() {
            return;
        }

        let cfg = Cfg::from_blocks(&body.blocks);
        let dom_info = DominatorInfo::compute(cfg.graph());
        let analysis = AllocaAnalysis::from_blocks(&body.blocks);
        let promotable = analysis.promotable_vars(&dom_info);

        if !promotable.is_empty() {
            let mut phis = PhiPlacement::new(cfg.num_blocks());
            Self::place_phis(&promotable, &mut phis, &cfg, &dom_info, &mut body.next_vreg);

            let mut renamer = Renamer::new(
                &body.blocks,
                &cfg,
                &dom_info,
                &mut phis,
                promotable.keys().copied().collect(),
            );
            renamer.run();
            body.blocks = renamer.finish();
        }
    }
}

impl Mem2RegPass {
    fn place_phis(
        promotable: &HashMap<LocalId, VarUsage>,
        phi_placement: &mut PhiPlacement,
        cfg: &Cfg,
        dom_info: &DominatorInfo,
        next_vreg: &mut usize,
    ) {
        for (&var_id, info) in promotable.iter() {
            if !info.has_load {
                continue;
            }

            let n = cfg.num_blocks();
            let gen: Vec<bool> = (0..n)
                .map(|b| info.load_before_store_blocks.contains(&b))
                .collect();
            let kill: Vec<bool> = (0..n).map(|b| info.def_blocks.contains(&b)).collect();
            let liveness = BackwardLiveness::<bool>::compute(&gen, &kill, cfg.graph());

            let mut worklist: VecDeque<usize> = info.def_blocks.iter().copied().collect();

            while let Some(b) = worklist.pop_front() {
                for &y in dom_info.dominance_frontier(b) {
                    if !liveness.live_in[y] {
                        continue;
                    }
                    if phi_placement.has_phi(y, var_id) {
                        continue;
                    }

                    let id = LocalId(*next_vreg);
                    *next_vreg += 1;
                    let dst = Operand::from(Local::new(Dtype::I32, id));
                    phi_placement.insert_phi(y, var_id, dst);

                    if !info.def_blocks.contains(&y) {
                        worklist.push_back(y);
                    }
                }
            }
        }
    }
}

struct AllocaAnalysis {
    usage: HashMap<LocalId, VarUsage>,
}

impl AllocaAnalysis {
    /// Constructs an `AllocaAnalysis` by scanning all basic blocks.
    ///
    /// First identifies alloca instructions that allocate i32 pointers as
    /// promotion candidates, then analyzes their load/store usage patterns
    /// across all blocks.
    fn from_blocks(blocks: &[BasicBlock]) -> Self {
        let candidates = Self::collect_candidates(blocks);
        let usage = Self::analyze_usage(blocks, &candidates);
        Self { usage }
    }

    /// Returns the subset of analyzed variables that are safe to promote to SSA form.
    ///
    /// A variable is promotable if:
    /// 1. It has at least one store (otherwise there's nothing to promote).
    /// 2. It is not used in any invalid way (e.g., address taken for non-load/store).
    /// 3. Every block that loads before storing is dominated by at least one
    ///    definition block, ensuring reads always see a defined value.
    ///
    /// Single-definition variables are rate-limited to avoid exploding the
    /// register allocator's interference graph on stress tests.
    fn promotable_vars(&self, dom_info: &DominatorInfo) -> HashMap<LocalId, VarUsage> {
        let mut multi_def = HashMap::new();
        let mut single_def = HashMap::new();

        for (&var, info) in &self.usage {
            if info.invalid || !info.has_store {
                continue;
            }

            let mut ok = true;
            for &block in &info.load_before_store_blocks {
                let has_dom_def = info
                    .def_blocks
                    .iter()
                    .any(|&def_block| def_block != block && dom_info.dominates(def_block, block));
                if !has_dom_def {
                    ok = false;
                    break;
                }
            }

            if ok {
                if info.def_blocks.len() <= 1 {
                    single_def.insert(var, info.clone());
                } else {
                    multi_def.insert(var, info.clone());
                }
            }
        }

        // Promoting single-def variables (defined in exactly one block)
        // extends live ranges: the stored value stays live from its
        // definition until its last use, instead of being killed at
        // the store.  For functions with very many single-def locals
        // (e.g., stress tests with thousands of variables), this causes
        // the O(n²) register allocator's interference graph to explode.
        //
        // Promote single-def variables only when the count is manageable.
        const SINGLE_DEF_LIMIT: usize = 256;
        if single_def.len() <= SINGLE_DEF_LIMIT {
            multi_def.extend(single_def);
        }

        multi_def
    }

    /// Scans all blocks for alloca instructions that produce `*i32` pointers.
    ///
    /// Returns the set of [`LocalId`]s for these allocas. Only i32 pointer
    /// allocas are considered because the current implementation only
    /// supports promoting scalar integer values.
    fn collect_candidates(blocks: &[BasicBlock]) -> HashSet<LocalId> {
        let mut candidates = HashSet::new();
        for stmt in blocks.iter().flat_map(|block| block.stmts.iter()) {
            if let StmtInner::Alloca(a) = &stmt.inner {
                if let Some(id) = a.dst.local_id() {
                    if let Dtype::Pointer { pointee } = a.dst.dtype() {
                        if matches!(pointee.as_ref(), Dtype::I32) {
                            candidates.insert(id);
                        }
                    }
                }
            }
        }
        candidates
    }

    /// Analyzes how each candidate alloca variable is used across all blocks.
    ///
    /// For each candidate, tracks:
    /// - `def_blocks`: blocks containing stores (definitions).
    /// - `load_before_store_blocks`: blocks that load the variable before any
    ///   store within the same block (upward-exposed uses).
    /// - `has_store` / `has_load`: whether stores/loads exist at all.
    /// - `invalid`: set if the variable is used in a non-promotable way (e.g.,
    ///   passed to a call or used as a general operand rather than load/store).
    fn analyze_usage(
        blocks: &[BasicBlock],
        candidates: &HashSet<LocalId>,
    ) -> HashMap<LocalId, VarUsage> {
        let mut usage: HashMap<LocalId, VarUsage> = candidates
            .iter()
            .map(|&v| (v, VarUsage::default()))
            .collect();

        for (b_idx, block) in blocks.iter().enumerate() {
            let mut store_seen: HashSet<LocalId> = HashSet::new();

            for stmt in &block.stmts {
                for op_ref in stmt.operands() {
                    let Some(id) = op_ref.operand.local_id() else {
                        continue;
                    };
                    let Some(info) = usage.get_mut(&id) else {
                        continue;
                    };
                    match op_ref.role {
                        OperandRole::LoadPtr => {
                            if !store_seen.contains(&id) {
                                info.load_before_store_blocks.insert(b_idx);
                            }
                            info.has_load = true;
                        }
                        OperandRole::StorePtr => {
                            store_seen.insert(id);
                            info.has_store = true;
                            info.def_blocks.insert(b_idx);
                        }
                        OperandRole::Def => {}
                        OperandRole::Use => {
                            info.invalid = true;
                        }
                    }
                }
            }
        }

        usage
    }
}

#[derive(Clone, Default)]
struct VarUsage {
    def_blocks: HashSet<usize>,
    load_before_store_blocks: HashSet<usize>,
    has_store: bool,
    has_load: bool,
    invalid: bool,
}

#[derive(Clone)]
struct PhiInfo {
    var: LocalId,
    dst: Operand,
    incomings: Vec<(BlockLabel, Operand)>,
}

struct PhiPlacement {
    nodes: Vec<Vec<PhiInfo>>,
    lookup: Vec<HashMap<LocalId, usize>>,
}

impl PhiPlacement {
    fn new(num_blocks: usize) -> Self {
        Self {
            nodes: vec![Vec::new(); num_blocks],
            lookup: vec![HashMap::new(); num_blocks],
        }
    }

    fn insert_phi(&mut self, block: usize, var: LocalId, dst: Operand) {
        let phi = PhiInfo {
            var,
            dst,
            incomings: Vec::new(),
        };
        self.lookup[block].insert(var, self.nodes[block].len());
        self.nodes[block].push(phi);
    }

    fn has_phi(&self, block: usize, var: LocalId) -> bool {
        self.lookup[block].contains_key(&var)
    }

    fn phis_at(&self, block: usize) -> &[PhiInfo] {
        &self.nodes[block]
    }

    fn phis_at_mut(&mut self, block: usize) -> &mut [PhiInfo] {
        &mut self.nodes[block]
    }
}

struct Renamer<'a> {
    blocks: &'a [BasicBlock],
    cfg: &'a Cfg,
    dom_info: &'a DominatorInfo,
    phi_placement: &'a mut PhiPlacement,
    promoted: HashSet<LocalId>,
    var_stack: HashMap<LocalId, Vec<Operand>>,
    alias_map: HashMap<LocalId, Operand>,
    rewritten: Vec<Vec<Stmt>>,
}

impl<'a> Renamer<'a> {
    fn new(
        blocks: &'a [BasicBlock],
        cfg: &'a Cfg,
        dom_info: &'a DominatorInfo,
        phi_placement: &'a mut PhiPlacement,
        promoted: HashSet<LocalId>,
    ) -> Self {
        let mut var_stack = HashMap::new();
        for var in promoted.iter().copied() {
            var_stack.insert(var, Vec::new());
        }
        Self {
            blocks,
            cfg,
            dom_info,
            phi_placement,
            promoted,
            var_stack,
            alias_map: HashMap::new(),
            rewritten: vec![Vec::new(); blocks.len()],
        }
    }

    fn run(&mut self) {
        for root in self.dom_info.dom_tree_roots() {
            self.clear_state();
            self.rename_block(root);
        }
    }

    fn finish(self) -> Vec<BasicBlock> {
        let mut out = Vec::with_capacity(self.blocks.len());

        for (i, block) in self.blocks.iter().enumerate() {
            let mut stmts = Vec::new();
            for phi in self.phi_placement.phis_at(i) {
                stmts.push(Stmt::as_phi(phi.dst.clone(), phi.incomings.clone()));
            }
            stmts.extend(self.rewritten[i].iter().cloned());
            out.push(BasicBlock {
                label: block.label.clone(),
                stmts,
            });
        }

        out
    }

    fn rename_block(&mut self, block_idx: usize) {
        let mut pushed_vars: Vec<LocalId> = Vec::new();
        let mut added_aliases: Vec<LocalId> = Vec::new();

        for phi in self.phi_placement.phis_at(block_idx) {
            if let Some(stack) = self.var_stack.get_mut(&phi.var) {
                stack.push(phi.dst.clone());
                pushed_vars.push(phi.var);
            }
        }

        for stmt in &self.blocks[block_idx].stmts {
            match &stmt.inner {
                StmtInner::Alloca(a) => {
                    if let Some(id) = a.dst.local_id() {
                        if self.promoted.contains(&id) {
                            continue;
                        }
                    }
                    self.rewritten[block_idx].push(stmt.clone());
                }
                StmtInner::Store(s) => {
                    if let Some(ptr_id) = s.ptr.local_id() {
                        if self.promoted.contains(&ptr_id) {
                            let src = self.resolve_alias(&s.src);
                            if let Some(stack) = self.var_stack.get_mut(&ptr_id) {
                                stack.push(src.clone());
                                pushed_vars.push(ptr_id);
                            }
                            continue;
                        }
                    }
                    let rewritten = self.rewrite_stmt(stmt);
                    self.rewritten[block_idx].push(rewritten);
                }
                StmtInner::Load(s) => {
                    if let Some(ptr_id) = s.ptr.local_id() {
                        if self.promoted.contains(&ptr_id) {
                            if let Some(dst_id) = s.dst.local_id() {
                                let cur = self.current_value(ptr_id);
                                self.alias_map.insert(dst_id, cur);
                                added_aliases.push(dst_id);
                            }
                            continue;
                        }
                    }
                    let rewritten = self.rewrite_stmt(stmt);
                    self.rewritten[block_idx].push(rewritten);
                }
                _ => {
                    let rewritten = self.rewrite_stmt(stmt);
                    self.rewritten[block_idx].push(rewritten);
                }
            }
        }

        let pred_label = self.cfg.label(block_idx).clone();
        for &succ in self.cfg.successors(block_idx) {
            let incoming_vals: Vec<Operand> = self
                .phi_placement
                .phis_at(succ)
                .iter()
                .map(|phi| self.current_value(phi.var))
                .collect();

            for (phi, val) in self
                .phi_placement
                .phis_at_mut(succ)
                .iter_mut()
                .zip(incoming_vals)
            {
                phi.incomings.push((pred_label.clone(), val));
            }
        }

        let children: Vec<usize> = self.dom_info.dom_children(block_idx).to_vec();
        for child in children {
            self.rename_block(child);
        }

        for idx in added_aliases {
            self.alias_map.remove(&idx);
        }
        for var in pushed_vars.into_iter().rev() {
            if let Some(stack) = self.var_stack.get_mut(&var) {
                stack.pop();
            }
        }
    }

    fn clear_state(&mut self) {
        for stack in self.var_stack.values_mut() {
            stack.clear();
        }
        self.alias_map.clear();
    }

    fn current_value(&self, var: LocalId) -> Operand {
        self.var_stack
            .get(&var)
            .and_then(|stack| stack.last())
            .map(|v| self.resolve_alias(v))
            .unwrap_or_else(|| Operand::from(0))
    }

    fn resolve_alias(&self, op: &Operand) -> Operand {
        let mut cur = op.clone();
        loop {
            if let Operand::Local(l) = &cur {
                if let Some(next) = self.alias_map.get(&l.id) {
                    cur = next.clone();
                    continue;
                }
            }
            break;
        }
        cur
    }

    fn rewrite_stmt(&self, stmt: &Stmt) -> Stmt {
        stmt.map_use_operands(|op| self.resolve_alias(op))
    }
}
