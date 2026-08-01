//! Dominator analysis over a control-flow graph.
//!
//! [`DominatorInfo`] computes, for a [`Graph`]:
//!
//! - the immediate dominator of every block, using the algorithm from
//!   Cooper, Harvey, and Kennedy, "A Simple, Fast Dominance Algorithm"
//!   (2001) — queried via [`DominatorInfo::dominates`] and the dominator
//!   tree via [`DominatorInfo::dom_children`] and
//!   [`DominatorInfo::dom_tree_roots`];
//! - the dominance frontier of every block
//!   ([`DominatorInfo::dominance_frontier`]), used by the mem2reg pass to
//!   decide where phi functions must be placed.

use crate::common::graph::Graph;
use std::collections::HashSet;

pub struct DominatorInfo {
    idom: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
    frontiers: Vec<HashSet<usize>>,
}

impl DominatorInfo {
    pub fn compute(graph: &Graph) -> Self {
        let preds = graph.preds_vec();
        let succs = graph.succs_vec();

        let idom = Self::compute_idom(preds, succs);
        let children = Self::build_dom_tree(&idom);
        let frontiers = Self::compute_dominance_frontiers(succs, &idom, &children);

        Self {
            idom,
            children,
            frontiers,
        }
    }

    pub fn dominates(&self, dominator: usize, block: usize) -> bool {
        let mut cur = block;
        loop {
            if cur == dominator {
                return true;
            }
            match self.idom[cur] {
                Some(parent) => cur = parent,
                None => return false,
            }
        }
    }

    pub fn dom_children(&self, block: usize) -> &[usize] {
        &self.children[block]
    }

    pub fn dominance_frontier(&self, block: usize) -> &HashSet<usize> {
        &self.frontiers[block]
    }

    pub fn dom_tree_roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.idom
            .iter()
            .enumerate()
            .filter_map(|(i, idom)| if idom.is_none() { Some(i) } else { None })
    }

    /// Computes the immediate dominator of every block using the algorithm from
    /// Cooper, Harvey, and Kennedy, "A Simple, Fast Dominance Algorithm" (2001).
    ///
    /// **Definition.** Block `d` is the _immediate dominator_ (`idom`) of block
    /// `n` if `d` strictly dominates `n` and does not strictly dominate any
    /// other strict dominator of `n`.  In other words, `idom(n)` is the closest
    /// dominator of `n` in the dominator tree.  The entry block has no
    /// immediate dominator.
    ///
    /// The algorithm works as follows:
    ///
    /// 1. **Reverse postorder (RPO) numbering.**
    ///    Perform a DFS from the entry block and record blocks in reverse
    ///    postorder.  This guarantees that (in a reducible CFG) every block's
    ///    dominator appears earlier in the ordering, so one pass usually
    ///    suffices to reach the fixed point.
    ///
    /// 2. **Initialization.**
    ///    - `idom(entry) = entry` — a sentinel that anchors the tree.
    ///    - `idom(n) = None` for all other blocks.
    ///
    /// 3. **Fixed-point iteration.**  Traverse every non-entry block `b` in RPO:
    ///    - Among `b`'s predecessors whose `idom` is already known, pick the
    ///      first one as a tentative immediate dominator.
    ///    - Fold the remaining processed predecessors in with the `intersect`
    ///      helper: given two blocks, `intersect` walks both upward through
    ///      the current `idom` chain (using RPO indices to decide which side
    ///      to advance) until they meet.  The meeting point is the nearest
    ///      common dominator of the two blocks.
    ///    - If the newly computed `idom(b)` differs from the current one,
    ///      record the change and mark the pass as dirty.
    ///
    ///    Repeat until a full pass produces no changes.
    ///
    /// 4. **Clean-up.**  Reset `idom(entry) = None`, since the entry block has
    ///    no true immediate dominator (the sentinel was only needed by the
    ///    iteration).
    ///
    /// **Complexity.** For reducible CFGs the algorithm converges in a single
    /// pass, giving O(n) time.  In the worst case (irreducible CFGs) it may
    /// require O(n²) time, but this is rare in practice.
    fn compute_idom(preds: &[Vec<usize>], succs: &[Vec<usize>]) -> Vec<Option<usize>> {
        let n = succs.len();
        if n == 0 {
            return Vec::new();
        }

        let start = 0;
        let order = Self::reverse_postorder(succs, start);

        let mut rpo_index = vec![usize::MAX; n];
        for (i, &b) in order.iter().enumerate() {
            rpo_index[b] = i;
        }

        let mut idom = vec![None; n];
        idom[start] = Some(start);

        let mut changed = true;
        while changed {
            changed = false;
            for &b in order.iter().skip(1) {
                let mut new_idom: Option<usize> = None;
                for &p in &preds[b] {
                    if idom[p].is_none() {
                        continue;
                    }
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(cur) => Self::intersect(p, cur, &idom, &rpo_index),
                    });
                }
                if idom[b] != new_idom {
                    idom[b] = new_idom;
                    changed = true;
                }
            }
        }

        if start < n {
            idom[start] = None;
        }

        idom
    }

    fn intersect(
        mut b1: usize,
        mut b2: usize,
        idom: &[Option<usize>],
        rpo_index: &[usize],
    ) -> usize {
        while b1 != b2 {
            while rpo_index[b1] > rpo_index[b2] {
                b1 = idom[b1].expect("missing idom during intersect");
            }
            while rpo_index[b2] > rpo_index[b1] {
                b2 = idom[b2].expect("missing idom during intersect");
            }
        }
        b1
    }

    fn reverse_postorder(succs: &[Vec<usize>], start: usize) -> Vec<usize> {
        let n = succs.len();
        let mut visited = vec![false; n];
        let mut post = Vec::new();

        fn dfs(v: usize, succs: &[Vec<usize>], visited: &mut [bool], post: &mut Vec<usize>) {
            if visited[v] {
                return;
            }
            visited[v] = true;
            for &s in &succs[v] {
                dfs(s, succs, visited, post);
            }
            post.push(v);
        }

        dfs(start, succs, &mut visited, &mut post);
        post.reverse();
        post
    }

    fn build_dom_tree(idom: &[Option<usize>]) -> Vec<Vec<usize>> {
        let mut children = vec![Vec::new(); idom.len()];
        for (b, parent) in idom.iter().enumerate() {
            if let Some(p) = parent {
                children[*p].push(b);
            }
        }
        children
    }

    fn compute_dominance_frontiers(
        succs: &[Vec<usize>],
        idom: &[Option<usize>],
        dom_children: &[Vec<usize>],
    ) -> Vec<HashSet<usize>> {
        let n = succs.len();
        let mut df: Vec<HashSet<usize>> = vec![HashSet::new(); n];

        fn dfs(
            b: usize,
            succs: &[Vec<usize>],
            idom: &[Option<usize>],
            dom_children: &[Vec<usize>],
            df: &mut [HashSet<usize>],
        ) {
            for &s in &succs[b] {
                if idom[s] != Some(b) {
                    df[b].insert(s);
                }
            }
            for &c in &dom_children[b] {
                dfs(c, succs, idom, dom_children, df);
                let child_df = df[c].clone();
                for w in child_df {
                    if idom[w] != Some(b) {
                        df[b].insert(w);
                    }
                }
            }
        }

        for (b, parent) in idom.iter().enumerate() {
            if parent.is_none() {
                dfs(b, succs, idom, dom_children, &mut df);
            }
        }

        df
    }
}
