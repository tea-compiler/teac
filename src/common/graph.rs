//! Graph data structures and dataflow analysis utilities for control-flow graphs.
//!
//! This module provides:
//! - [`CfgNode`]: a trait for nodes in a control-flow graph.
//! - [`Graph`]: a directed graph with successor and predecessor adjacency lists.
//! - [`Lattice`]: a trait defining a lattice for dataflow analysis.
//! - [`BackwardLiveness`]: backward liveness analysis using a worklist algorithm.

use std::collections::{HashMap, HashSet, VecDeque};

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
/// - a bottom element (the initial / most-conservative value),
/// - a join (least upper bound) operation for merging values at join points,
/// - a transfer function that computes the inflow from the outflow using
///   gen/kill sets.
pub trait Lattice: Clone + PartialEq {
    /// Returns the bottom element of the lattice (the initial dataflow value).
    fn bottom() -> Self;

    /// Computes the least upper bound of `self` and `other` in place (join / merge).
    fn join(&mut self, other: &Self);

    /// Applies the transfer function: `gen ∪ (out ∖ kill)`.
    ///
    /// Returns the lattice value that flows into a node given
    /// - `gen`: values generated (defined / used) by the node,
    /// - `kill`: values killed (overwritten) by the node,
    /// - `out`: values live at the exit of the node.
    fn transfer(gen: &Self, kill: &Self, out: &Self) -> Self;
}

/// Simple single-bit reachability lattice.
///
/// `false` is the bottom element. `join` is logical OR. The transfer function
/// propagates liveness if the value is generated or passes through (live-out
/// and not killed).
impl Lattice for bool {
    fn bottom() -> Self {
        false
    }

    fn join(&mut self, other: &Self) {
        *self = *self || *other;
    }

    fn transfer(gen: &Self, kill: &Self, out: &Self) -> Self {
        *gen || (*out && !*kill)
    }
}

/// A set of virtual-register indices, used as the liveness lattice element
/// when tracking the live set of virtual registers.
#[derive(Clone, PartialEq, Eq)]
pub struct VregSet(pub HashSet<usize>);

/// Set-of-virtual-registers liveness lattice.
///
/// The bottom element is the empty set. `join` is set union. The transfer
/// function is `gen ∪ (out ∖ kill)`.
impl Lattice for VregSet {
    fn bottom() -> Self {
        VregSet(HashSet::new())
    }

    fn join(&mut self, other: &Self) {
        self.0.extend(other.0.iter().copied());
    }

    fn transfer(gen: &Self, kill: &Self, out: &Self) -> Self {
        let mut result = gen.0.clone();
        for v in &out.0 {
            if !kill.0.contains(v) {
                result.insert(*v);
            }
        }
        VregSet(result)
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
    /// The worklist is initially seeded with all nodes in reverse order so
    /// that nodes near the end of the CFG are processed first. Whenever
    /// `live_in[i]` changes, all predecessors of `i` are added back to the
    /// worklist to propagate the change backward until a fixed point is reached.
    pub fn compute(gen: &[L], kill: &[L], graph: &Graph) -> Self {
        let n = graph.num_nodes();

        let mut live_in: Vec<L> = (0..n).map(|_| L::bottom()).collect();
        let mut live_out: Vec<L> = (0..n).map(|_| L::bottom()).collect();

        let mut in_worklist = vec![true; n];
        let mut worklist: VecDeque<usize> = (0..n).rev().collect();

        while let Some(i) = worklist.pop_front() {
            in_worklist[i] = false;

            let mut new_out = L::bottom();
            for &s in graph.successors(i) {
                new_out.join(&live_in[s]);
            }

            let new_in = L::transfer(&gen[i], &kill[i], &new_out);

            if new_in != live_in[i] {
                live_in[i] = new_in;
                live_out[i] = new_out;

                for &p in graph.predecessors(i) {
                    if !in_worklist[p] {
                        in_worklist[p] = true;
                        worklist.push_back(p);
                    }
                }
            } else {
                live_out[i] = new_out;
            }
        }

        Self { live_in, live_out }
    }
}
