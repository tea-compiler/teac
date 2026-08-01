//! Control-flow graph wrapper over the basic blocks of a function body.
//!
//! This module provides:
//! - the [`CfgNode`] implementation for [`BasicBlock`], deriving successor
//!   edges from each block's terminator: [`StmtInner::Jump`] and
//!   [`StmtInner::CJump`] target their named blocks, [`StmtInner::Return`]
//!   has no successors, and any other (or missing) terminator falls through
//!   to the next block in layout order.
//! - [`Cfg`]: a thin wrapper pairing the block labels with the [`Graph`]
//!   built from them, so analyses can address blocks by index and map them
//!   back to IR labels.
//!
//! `Cfg` lives in `common` — alongside [`Graph`] — because it is consumed
//! by both the middle end ([`crate::opt`]) and the backend (`crate::asm`);
//! keeping it below both layers avoids a backend-to-middle-end dependency.

use super::graph::{CfgNode, Graph};
use crate::ir::function::{BasicBlock, BlockLabel};
use crate::ir::stmt::StmtInner;
use std::collections::HashMap;

/// Marks an IR [`BasicBlock`] as a control-flow graph node so that
/// [`Graph::from_nodes`] can build a graph directly over a function body.
///
/// The block's label serves as its name for branch-target resolution;
/// successor edges are taken from the block's terminator, defaulting to
/// fall-through to the next block in the slice.
impl CfgNode for BasicBlock {
    fn label(&self) -> Option<String> {
        Some(self.label.key())
    }

    fn successors(
        &self,
        idx: usize,
        num_nodes: usize,
        label_map: &HashMap<String, usize>,
    ) -> Vec<usize> {
        let term = self.stmts.last();

        match term.map(|s| &s.inner) {
            Some(StmtInner::Jump(j)) => vec![label_map[&j.target.key()]],
            Some(StmtInner::CJump(j)) => vec![
                label_map[&j.true_label.key()],
                label_map[&j.false_label.key()],
            ],
            Some(StmtInner::Return(_)) => Vec::new(),
            _ => {
                if idx + 1 < num_nodes {
                    vec![idx + 1]
                } else {
                    Vec::new()
                }
            }
        }
    }
}

/// A control-flow graph over the basic blocks of a function body.
///
/// Blocks are addressed by their index in the slice passed to
/// [`Cfg::from_blocks`]; `labels` preserves the corresponding [`BlockLabel`]
/// of each block so that graph results can be mapped back onto the IR.
pub struct Cfg {
    labels: Vec<BlockLabel>,
    graph: Graph,
}

impl Cfg {
    /// Builds a [`Cfg`] from the basic blocks of a function body.
    ///
    /// The order of `blocks` becomes the index space of the graph; edges
    /// are derived from each block's terminator (see the [`CfgNode`]
    /// implementation for [`BasicBlock`]).
    pub fn from_blocks(blocks: &[BasicBlock]) -> Self {
        let labels: Vec<BlockLabel> = blocks.iter().map(|b| b.label.clone()).collect();
        let graph = Graph::from_nodes(blocks);
        Self { labels, graph }
    }

    /// Returns the underlying [`Graph`], for analyses that work on the raw
    /// adjacency lists.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Returns the number of basic blocks in the graph.
    pub fn num_blocks(&self) -> usize {
        self.graph.num_nodes()
    }

    /// Returns the successor indices of `block`.
    pub fn successors(&self, block: usize) -> &[usize] {
        self.graph.successors(block)
    }

    /// Returns the predecessor indices of `block`.
    pub fn predecessors(&self, block: usize) -> &[usize] {
        self.graph.predecessors(block)
    }

    /// Returns the label of `block`.
    pub fn label(&self, block: usize) -> &BlockLabel {
        &self.labels[block]
    }

    /// Returns the labels of all blocks, in index order.
    pub fn labels(&self) -> &[BlockLabel] {
        &self.labels
    }
}
