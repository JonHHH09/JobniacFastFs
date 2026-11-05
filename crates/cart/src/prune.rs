//! Minimal cost-complexity (weakest-link) pruning, Breiman et al. 1984, chapter 3.
//!
//! The node cost is the impurity-based one used by scikit-learn's `ccp_alpha`, not the
//! misclassification rate: `R(t) = (W_t / W_root) * i(t)` where `W_t` is the node's total weight,
//! `i` the tree's criterion (Gini or entropy) applied to the node's weighted counts, and the cost
//! of a subtree `R(T_t)` is the sum of `R` over its leaves. The effective alpha of an internal node
//! is `(R(t) - R(T_t)) / (|leaves(T_t)| - 1)`. Pruning repeatedly collapses the node with the
//! smallest effective alpha (ties: lowest node index) and recomputes. Each round is `O(nodes)`, so
//! a whole path costs `O(nodes^2)`.

use crate::error::CartError;
use crate::tree::{Node, Split, Tree};

/// One step of the weakest-link sequence: collapse `node` at (monotone-clamped) effective `alpha`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Link {
    pub(crate) alpha: f64,
    pub(crate) node: usize,
}

/// Runs weakest-link pruning while the (monotone-clamped) effective alpha is at most `limit`.
/// Returns the steps in order; their alphas are non-decreasing.
pub(crate) fn weakest_links(tree: &Tree, limit: f64) -> Vec<Link> {
    let nodes = &tree.nodes;
    let n = nodes.len();
    let criterion = tree.config.criterion;
    let root_weight = nodes.first().map_or(0.0, Node::weight);
    let cost: Vec<f64> = nodes
        .iter()
        .map(|node| {
            if root_weight > 0.0 {
                node.weight() / root_weight * criterion.impurity(&node.counts)
            } else {
                0.0
            }
        })
        .collect();
    let mut collapsed = vec![false; n];
    let mut links: Vec<Link> = Vec::new();
    let mut last = 0.0_f64;
    loop {
        // Subtree leaf count and cost, children first.
        let mut leaves = vec![1usize; n];
        let mut subtree_cost = cost.clone();
        for i in (0..n).rev() {
            if collapsed[i] {
                continue;
            }
            if let Some(Split { left, right, .. }) = &nodes[i].split {
                leaves[i] = leaves[*left] + leaves[*right];
                subtree_cost[i] = subtree_cost[*left] + subtree_cost[*right];
            }
        }
        // Reachability from the root through uncollapsed splits, parents first.
        let mut reachable = vec![false; n];
        if n > 0 {
            reachable[0] = true;
        }
        let mut weakest: Option<(usize, f64)> = None;
        for i in 0..n {
            if !reachable[i] || collapsed[i] {
                continue;
            }
            let Some(Split { left, right, .. }) = &nodes[i].split else {
                continue;
            };
            reachable[*left] = true;
            reachable[*right] = true;
            let alpha = ((cost[i] - subtree_cost[i]) / (leaves[i] - 1) as f64).max(0.0);
            if weakest.is_none_or(|(_, best)| alpha < best) {
                weakest = Some((i, alpha));
            }
        }
        let Some((node, alpha)) = weakest else { break };
        let alpha = alpha.max(last);
        if alpha > limit {
            break;
        }
        links.push(Link { alpha, node });
        last = alpha;
        collapsed[node] = true;
    }
    links
}

/// The effective alphas at which the tree changes, in increasing order.
///
/// The list starts with `0.0` (the unpruned tree; note `prune(tree, 0.0)` still collapses links
/// whose effective alpha is exactly 0, i.e. splits that gained nothing) and then holds each distinct effective alpha of
/// the weakest-link sequence, so `prune(tree, a)` for `a` in the list yields the nested sequence of
/// subtrees `T_0 ⊇ T_1 ⊇ ... ⊇ {root}`. The last entry is the alpha at which the tree collapses to
/// its root; a tree that is already a single leaf gives `[0.0]`. See the module docs for the cost
/// definition (impurity-based, as in scikit-learn).
#[must_use]
pub fn cost_complexity_path(tree: &Tree) -> Vec<f64> {
    let mut path = vec![0.0];
    for Link { alpha, .. } in weakest_links(tree, f64::INFINITY) {
        if path.last().is_some_and(|&last| alpha > last) {
            path.push(alpha);
        }
    }
    path
}

/// Returns the subtree left after collapsing every weakest link whose effective alpha is at most
/// `alpha`. `prune(tree, 0.0)` only removes splits that gained nothing; a huge alpha leaves the root
/// as a single leaf. The result is renumbered so children still have larger indices than parents.
///
/// # Errors
/// `alpha` must be finite and not negative.
pub fn prune(tree: &Tree, alpha: f64) -> Result<Tree, CartError> {
    if !(alpha.is_finite() && alpha >= 0.0) {
        return Err(CartError::InvalidArgument {
            reason: format!("alpha {alpha} must be finite and not negative"),
        });
    }
    let links = weakest_links(tree, alpha);
    Ok(prune_links(tree, &links))
}

/// Collapses the nodes of `links` (a prefix of a weakest-link sequence) and renumbers the rest.
pub(crate) fn prune_links(tree: &Tree, links: &[Link]) -> Tree {
    let mut collapsed = vec![false; tree.nodes.len()];
    for link in links {
        collapsed[link.node] = true;
    }
    let mut nodes: Vec<Node> = Vec::new();
    if let Some(root) = tree.nodes.first() {
        nodes.push(Node {
            counts: root.counts.clone(),
            samples: root.samples,
            split: None,
        });
        let mut stack = vec![(0usize, 0usize)]; // (old index, new index)
        while let Some((old, new)) = stack.pop() {
            let Some(Split {
                feature,
                threshold,
                left,
                right,
            }) = tree.nodes[old].split.as_ref().filter(|_| !collapsed[old])
            else {
                continue;
            };
            let new_left = nodes.len();
            let new_right = new_left + 1;
            for &child in &[*left, *right] {
                let source = &tree.nodes[child];
                nodes.push(Node {
                    counts: source.counts.clone(),
                    samples: source.samples,
                    split: None,
                });
            }
            nodes[new].split = Some(Split {
                feature: *feature,
                threshold: *threshold,
                left: new_left,
                right: new_right,
            });
            stack.push((*right, new_right));
            stack.push((*left, new_left));
        }
    }
    Tree {
        feature_names: tree.feature_names.clone(),
        class_names: tree.class_names.clone(),
        config: tree.config.clone(),
        nodes,
    }
}
