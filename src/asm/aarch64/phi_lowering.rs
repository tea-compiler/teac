use std::collections::HashMap;

use super::function_generator::FunctionGenerator;
use crate::asm::error::Error;
use crate::ir::function::{BasicBlock, BlockLabel};
use crate::ir::stmt::{PhiStmt, Stmt, StmtInner};
use crate::ir::{Local, LocalId, Operand};
use crate::opt::cfg::Cfg;

pub fn lower_function_blocks(
    ctx: &mut FunctionGenerator<'_>,
    blocks: &[BasicBlock],
) -> Result<(), Error> {
    let mut parsed: Vec<ParsedBlock> = blocks.iter().map(ParsedBlock::from_block).collect();
    let cfg = Cfg::from_blocks(blocks);
    let mut edges = EdgeCopies::new(next_basic_block_id(cfg.labels()));

    place_phi_copies(&parsed, &cfg, &mut edges);
    edges.patch_terminators(&mut parsed, cfg.labels());
    assemble(ctx, parsed, cfg.labels(), &edges)
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
                edges.split(pred_idx, block_idx, copies);
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

fn assemble(
    ctx: &mut FunctionGenerator<'_>,
    parsed: Vec<ParsedBlock>,
    labels: &[BlockLabel],
    edges: &EdgeCopies,
) -> Result<(), Error> {
    for (idx, block) in parsed.into_iter().enumerate() {
        ctx.emit_label(&block.label);
        emit_body_with_copies(
            ctx,
            &block.body,
            edges.pending_inserts.get(&idx).map(Vec::as_slice),
        )?;
    }

    edges.materialize_splits(ctx, labels)
}

fn emit_body_with_copies(
    ctx: &mut FunctionGenerator<'_>,
    body: &[Stmt],
    copies: Option<&[ParallelCopy]>,
) -> Result<(), Error> {
    let term_pos = body.iter().rposition(is_terminator);

    match term_pos {
        Some(pos) => {
            for stmt in &body[..pos] {
                ctx.emit_stmt(stmt)?;
            }
            emit_parallel_copies(ctx, copies)?;
            for stmt in &body[pos..] {
                ctx.emit_stmt(stmt)?;
            }
        }
        None => {
            for stmt in body {
                ctx.emit_stmt(stmt)?;
            }
            emit_parallel_copies(ctx, copies)?;
        }
    }

    Ok(())
}

fn emit_parallel_copies(
    ctx: &mut FunctionGenerator<'_>,
    copies: Option<&[ParallelCopy]>,
) -> Result<(), Error> {
    let Some(copies) = copies else {
        return Ok(());
    };

    let mut pending = copies.to_vec();
    while !pending.is_empty() {
        if let Some(idx) = find_ready_copy(&pending) {
            let copy = pending.remove(idx);
            ctx.emit_copy(&copy.dst, &copy.src)?;
            continue;
        }

        let cycle_dst = pending[0].dst.clone();
        let temp = Operand::from(Local::new(
            cycle_dst.dtype().clone(),
            LocalId(ctx.fresh_vreg()),
        ));
        ctx.emit_copy(&temp, &cycle_dst)?;

        for copy in &mut pending {
            if same_operand(&copy.src, &cycle_dst) {
                copy.src = temp.clone();
            }
        }
    }

    Ok(())
}

fn find_ready_copy(copies: &[ParallelCopy]) -> Option<usize> {
    copies.iter().position(|copy| {
        !copies
            .iter()
            .any(|other| same_operand(&copy.dst, &other.src))
    })
}

fn is_terminator(stmt: &Stmt) -> bool {
    matches!(
        stmt.inner,
        StmtInner::Jump(_) | StmtInner::CJump(_) | StmtInner::Return(_)
    )
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

fn same_operand(lhs: &Operand, rhs: &Operand) -> bool {
    match (lhs, rhs) {
        (Operand::Const(l), Operand::Const(r)) => l.val == r.val,
        (Operand::Local(l), Operand::Local(r)) => l.id == r.id,
        (Operand::Global(l), Operand::Global(r)) => l.name == r.name,
        _ => false,
    }
}

#[derive(Clone)]
struct ParallelCopy {
    dst: Operand,
    src: Operand,
}

struct ParsedBlock {
    label: BlockLabel,
    phis: Vec<PhiStmt>,
    body: Vec<Stmt>,
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

struct SplitEdge {
    pred: usize,
    succ: usize,
    label: BlockLabel,
    copies: Vec<ParallelCopy>,
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

    fn split(&mut self, pred: usize, succ: usize, copies: Vec<ParallelCopy>) {
        let label = BlockLabel::BasicBlock(self.next_block_id);
        self.next_block_id += 1;

        self.splits.push(SplitEdge {
            pred,
            succ,
            label,
            copies,
        });
    }

    fn insert_at_pred(&mut self, pred: usize, copies: Vec<ParallelCopy>) {
        self.pending_inserts.entry(pred).or_default().extend(copies);
    }

    fn patch_terminators(&self, blocks: &mut [ParsedBlock], labels: &[BlockLabel]) {
        for split in &self.splits {
            let target_key = labels[split.succ].key();
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

    fn materialize_splits(
        &self,
        ctx: &mut FunctionGenerator<'_>,
        labels: &[BlockLabel],
    ) -> Result<(), Error> {
        for split in &self.splits {
            ctx.emit_label(&split.label);
            emit_parallel_copies(ctx, Some(&split.copies))?;
            ctx.emit_stmt(&Stmt::as_jump(labels[split.succ].clone()))?;
        }

        Ok(())
    }
}
