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

    /// Computes the immediate dominator (`idom`) of every block with the
    /// algorithm of Cooper, Harvey, and Kennedy, "A Simple, Fast Dominance
    /// Algorithm" (2001): sweep non-entry blocks in reverse postorder, fold
    /// each block's already-processed predecessors into their nearest common
    /// dominator via `intersect`, and repeat until a full pass changes
    /// nothing.
    ///
    /// Reverse postorder visits a block's dominators before the block itself
    /// in a reducible CFG, so the fixed point is reached after few passes.
    /// The `idom[start] = start` sentinel anchors the chain walks in
    /// `intersect`; it is reset to `None` after the loop because the entry
    /// block has no immediate dominator. Blocks unreachable from `start`
    /// never enter the RPO and keep `idom = None`, which is why the
    /// dominator tree can have several roots.
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

    /// Returns the nearest common dominator of `b1` and `b2` by walking both
    /// up the `idom` chain until they meet. Every `idom` step lowers a
    /// block's RPO number, so advancing the higher-numbered block always
    /// steps toward the meeting point.
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
