//! Graph data structures and dataflow analysis utilities for control-flow graphs.
//!
//! This module provides:
//! - [`CfgNode`]: a trait for nodes in a control-flow graph.
//! - [`Graph`]: a directed graph with successor and predecessor adjacency lists.
//! - [`Lattice`]: a trait defining a lattice for dataflow analysis, implemented
//!   for both [`bool`] (cheap per-variable analyses such as phi placement) and
//!   [`Bitset`] (large dense live sets used by the register allocator).
//! - [`BackwardLiveness`]: backward liveness analysis using a worklist
//!   algorithm, generic over any [`Lattice`] type.

use super::bitset::Bitset;
use std::collections::{HashMap, VecDeque};

/// A node in a control-flow graph (CFG).
///
/// Implementors describe how each node connects to its successors and
/// optionally expose a label so that branch targets can be resolved by name.
pub trait CfgNode {
    /// Returns an optional label for this node.
    ///
    /// When present, the label is used to build a name-to-index map so that
    /// other nodes can refer to this node as a branch target by name.
    fn label(&self) -> Option<String>;

    /// Computes the successor node indices for this node.
    ///
    /// - `idx`: the index of this node in the owning node slice.
    /// - `num_nodes`: total number of nodes in the graph.
    /// - `label_map`: a map from label strings to node indices, used to
    ///   resolve named branch targets.
    fn successors(
        &self,
        idx: usize,
        num_nodes: usize,
        label_map: &HashMap<String, usize>,
    ) -> Vec<usize>;
}

/// A directed graph represented as both successor and predecessor adjacency lists.
///
/// Both lists are indexed by node index and are derived from the same edge set,
/// so they are always consistent with each other.
pub struct Graph {
    succs: Vec<Vec<usize>>,
    preds: Vec<Vec<usize>>,
}

impl Graph {
    /// Constructs a [`Graph`] from a pre-built successor adjacency list.
    ///
    /// The predecessor adjacency list is derived automatically by inverting
    /// the edges of `succs`.
    pub fn new(succs: Vec<Vec<usize>>) -> Self {
        let n = succs.len();
        let mut preds = vec![Vec::new(); n];
        for (i, succ_list) in succs.iter().enumerate() {
            for &s in succ_list {
                preds[s].push(i);
            }
        }
        Self { succs, preds }
    }

    /// Builds a [`Graph`] from a slice of [`CfgNode`] implementors.
    ///
    /// This method first collects all node labels into a name-to-index map,
    /// then calls [`CfgNode::successors`] on each node to compute the full
    /// successor adjacency list, and finally delegates to [`Graph::new`].
    pub fn from_nodes<N: CfgNode>(nodes: &[N]) -> Self {
        let n = nodes.len();
        let label_map: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .filter_map(|(i, node)| node.label().map(|k| (k, i)))
            .collect();
        let succs = nodes
            .iter()
            .enumerate()
            .map(|(i, node)| node.successors(i, n, &label_map))
            .collect();
        Self::new(succs)
    }

    /// Returns the total number of nodes in the graph.
    pub fn num_nodes(&self) -> usize {
        self.succs.len()
    }

    /// Returns the successor indices of `node`.
    pub fn successors(&self, node: usize) -> &[usize] {
        &self.succs[node]
    }

    /// Returns the predecessor indices of `node`.
    pub fn predecessors(&self, node: usize) -> &[usize] {
        &self.preds[node]
    }

    /// Returns the full successor adjacency list.
    pub fn succs_vec(&self) -> &[Vec<usize>] {
        &self.succs
    }

    /// Returns the full predecessor adjacency list.
    pub fn preds_vec(&self) -> &[Vec<usize>] {
        &self.preds
    }
}

/// A lattice used as the value domain for dataflow analysis.
///
/// Each implementor defines:
/// - a reset-to-bottom operation that brings the value to the most
///   conservative element while preserving any size context (e.g., a bit
///   set's capacity),
/// - a join (least upper bound) operation for merging values at join points,
/// - a transfer function that updates the value in place to `gen ∪ (out ∖ kill)`.
///
/// All in-place operations return whether the value actually changed so the
/// worklist algorithm can detect convergence without extra equality checks.
pub trait Lattice: Clone + PartialEq {
    /// Resets `self` to the bottom element while preserving any internal
    /// size context (e.g., a [`Bitset`]'s capacity).
    fn reset(&mut self);

    /// In-place least upper bound: `self ⊔= other`.  Returns `true` if
    /// `self` changed.
    fn join(&mut self, other: &Self) -> bool;

    /// In-place transfer function: `self = gen ∪ (out ∖ kill)`.  Returns
    /// `true` if `self` changed.
    fn transfer(&mut self, gen: &Self, kill: &Self, out: &Self) -> bool;
}

/// Single-bit reachability lattice used for per-variable phi placement.
///
/// `false` is the bottom element; `join` is logical OR; the transfer function
/// is `gen ∨ (out ∧ ¬kill)`.
impl Lattice for bool {
    fn reset(&mut self) {
        *self = false;
    }

    fn join(&mut self, other: &Self) -> bool {
        if *other && !*self {
            *self = true;
            true
        } else {
            false
        }
    }

    fn transfer(&mut self, gen: &Self, kill: &Self, out: &Self) -> bool {
        let new = *gen || (*out && !*kill);
        if new != *self {
            *self = new;
            true
        } else {
            false
        }
    }
}

/// Bit-set lattice over a dense index domain (e.g., virtual registers).
///
/// `Bitset::new(capacity)` is the bottom element; `join` is word-parallel
/// union; the transfer function is `gen | (out & !kill)` computed word by
/// word.  All operations require both operands to share the same capacity.
impl Lattice for Bitset {
    fn reset(&mut self) {
        self.clear();
    }

    fn join(&mut self, other: &Self) -> bool {
        self.union_with(other)
    }

    fn transfer(&mut self, gen: &Self, kill: &Self, out: &Self) -> bool {
        self.assign_transfer(gen, kill, out)
    }
}

/// Results of backward liveness (dataflow) analysis over a [`Graph`].
///
/// - `live_in[i]` holds the lattice value live at the **entry** of node `i`.
/// - `live_out[i]` holds the lattice value live at the **exit** of node `i`.
pub struct BackwardLiveness<L> {
    /// Lattice values live at the entry of each node.
    pub live_in: Vec<L>,
    /// Lattice values live at the exit of each node.
    pub live_out: Vec<L>,
}

impl<L: Lattice> BackwardLiveness<L> {
    /// Performs backward liveness analysis using a worklist algorithm.
    ///
    /// `bottom` supplies the initial value at every node; it must already be
    /// at the lattice's bottom element and carry any required size context
    /// (e.g., a [`Bitset`]'s capacity).
    ///
    /// The worklist is initially seeded with all nodes in reverse order so
    /// that nodes near the end of the CFG are processed first.  Whenever
    /// `live_in[i]` changes, all predecessors of `i` are added back to the
    /// worklist to propagate the change backward until a fixed point is
    /// reached.
    pub fn compute(gen: &[L], kill: &[L], graph: &Graph, bottom: L) -> Self {
        let n = graph.num_nodes();
        debug_assert_eq!(gen.len(), n);
        debug_assert_eq!(kill.len(), n);

        let mut live_in: Vec<L> = vec![bottom.clone(); n];
        let mut live_out: Vec<L> = vec![bottom.clone(); n];

        let mut in_worklist = vec![true; n];
        let mut worklist: VecDeque<usize> = (0..n).rev().collect();
        let mut new_out = bottom;

        while let Some(i) = worklist.pop_front() {
            in_worklist[i] = false;

            new_out.reset();
            for &s in graph.successors(i) {
                new_out.join(&live_in[s]);
            }

            let in_changed = live_in[i].transfer(&gen[i], &kill[i], &new_out);
            std::mem::swap(&mut live_out[i], &mut new_out);

            if in_changed {
                for &p in graph.predecessors(i) {
                    if !in_worklist[p] {
                        in_worklist[p] = true;
                        worklist.push_back(p);
                    }
                }
            }
        }

        Self { live_in, live_out }
    }
}
