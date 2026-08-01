use std::collections::HashMap;

use crate::ir::function::{BasicBlock, BlockLabel};
use crate::ir::stmt::{PhiStmt, Stmt, StmtInner};
use crate::common::cfg::Cfg;
use crate::ir::Operand;

/// Plan for destroying SSA form in one function: the block bodies with
/// their phi nodes stripped and terminators retargeted across split
/// edges, the parallel copies to inject before each block's terminator,
/// and the freshly synthesised split-edge blocks.  The plan is pure
/// data; the function generator materialises it into instructions.
pub struct LoweringPlan {
    pub blocks: Vec<ParsedBlock>,
    pub pending_inserts: HashMap<usize, Vec<ParallelCopy>>,
    pub splits: Vec<SplitEdge>,
}

/// Computes the SSA-destruction plan for `blocks`: places phi copies on
/// each control-flow edge, splitting critical edges where a predecessor
/// has multiple successors, and rewrites terminators to target the split
/// blocks.  Performs no emission and never touches the generator.
pub fn plan(blocks: &[BasicBlock]) -> LoweringPlan {
    let mut parsed: Vec<ParsedBlock> = blocks.iter().map(ParsedBlock::from_block).collect();
    let cfg = Cfg::from_blocks(blocks);
    let mut edges = EdgeCopies::new(next_basic_block_id(cfg.labels()));

    place_phi_copies(&parsed, &cfg, &mut edges);
    edges.patch_terminators(&mut parsed);

    LoweringPlan {
        blocks: parsed,
        pending_inserts: edges.pending_inserts,
        splits: edges.splits,
    }
}

fn place_phi_copies(parsed: &[ParsedBlock], cfg: &Cfg, edges: &mut EdgeCopies) {
    for (block_idx, block) in parsed.iter().enumerate() {
        if block.phis.is_empty() {
            continue;
        }

        for &pred_idx in cfg.predecessors(block_idx) {
            let copies = build_parallel_copies(&block.phis, cfg.label(pred_idx));
            if copies.is_empty() {
                continue;
            }

            if cfg.successors(pred_idx).len() == 1 {
                edges.insert_at_pred(pred_idx, copies);
            } else {
                edges.split(pred_idx, cfg.label(block_idx).clone(), copies);
            }
        }
    }
}

fn build_parallel_copies(phis: &[PhiStmt], pred_label: &BlockLabel) -> Vec<ParallelCopy> {
    let pred_key = pred_label.key();

    phis.iter()
        .map(|phi| {
            let src = phi
                .incomings
                .iter()
                .find(|(label, _)| label.key() == pred_key)
                .map(|(_, val)| val.clone())
                .unwrap_or_else(|| Operand::from(0));

            ParallelCopy {
                dst: phi.dst.clone(),
                src,
            }
        })
        .filter(|copy| !same_operand(&copy.dst, &copy.src))
        .collect()
}

/// Index of the first ready copy: one whose destination is not the source
/// of any other pending copy, so emitting it cannot clobber a value that
/// is still needed.  `None` means the remaining copies form a cycle.
pub fn find_ready_copy(copies: &[ParallelCopy]) -> Option<usize> {
    copies.iter().position(|copy| {
        !copies
            .iter()
            .any(|other| same_operand(&copy.dst, &other.src))
    })
}

pub fn same_operand(lhs: &Operand, rhs: &Operand) -> bool {
    match (lhs, rhs) {
        (Operand::Const(l), Operand::Const(r)) => l.val == r.val,
        (Operand::Local(l), Operand::Local(r)) => l.id == r.id,
        (Operand::Global(l), Operand::Global(r)) => l.name == r.name,
        _ => false,
    }
}

fn next_basic_block_id(labels: &[BlockLabel]) -> usize {
    labels
        .iter()
        .filter_map(|label| match label {
            BlockLabel::BasicBlock(n) => Some(*n + 1),
            _ => None,
        })
        .max()
        .unwrap_or(1)
}

#[derive(Clone)]
pub struct ParallelCopy {
    pub dst: Operand,
    pub src: Operand,
}

pub struct ParsedBlock {
    pub label: BlockLabel,
    phis: Vec<PhiStmt>,
    pub body: Vec<Stmt>,
}

impl ParsedBlock {
    fn from_block(block: &BasicBlock) -> Self {
        let mut phis = Vec::new();
        let mut body = Vec::new();

        for stmt in &block.stmts {
            match &stmt.inner {
                StmtInner::Phi(phi) => phis.push(phi.clone()),
                _ => body.push(stmt.clone()),
            }
        }

        Self {
            label: block.label.clone(),
            phis,
            body,
        }
    }
}

pub struct SplitEdge {
    pred: usize,
    pub succ_label: BlockLabel,
    pub label: BlockLabel,
    pub copies: Vec<ParallelCopy>,
}

struct EdgeCopies {
    splits: Vec<SplitEdge>,
    pending_inserts: HashMap<usize, Vec<ParallelCopy>>,
    next_block_id: usize,
}

impl EdgeCopies {
    fn new(next_block_id: usize) -> Self {
        Self {
            splits: Vec::new(),
            pending_inserts: HashMap::new(),
            next_block_id,
        }
    }

    fn split(&mut self, pred: usize, succ_label: BlockLabel, copies: Vec<ParallelCopy>) {
        let label = BlockLabel::BasicBlock(self.next_block_id);
        self.next_block_id += 1;

        self.splits.push(SplitEdge {
            pred,
            succ_label,
            label,
            copies,
        });
    }

    fn insert_at_pred(&mut self, pred: usize, copies: Vec<ParallelCopy>) {
        self.pending_inserts.entry(pred).or_default().extend(copies);
    }

    fn patch_terminators(&self, blocks: &mut [ParsedBlock]) {
        for split in &self.splits {
            let target_key = split.succ_label.key();
            if let Some(term) = blocks[split.pred].body.last_mut() {
                match &mut term.inner {
                    StmtInner::Jump(j) if j.target.key() == target_key => {
                        j.target = split.label.clone();
                    }
                    StmtInner::CJump(j) => {
                        if j.true_label.key() == target_key {
                            j.true_label = split.label.clone();
                        }
                        if j.false_label.key() == target_key {
                            j.false_label = split.label.clone();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
