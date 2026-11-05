//! The fitted tree: flat node storage, prediction and structural queries.

use crate::config::TrainConfig;
use crate::error::CartError;

/// Maximum number of nodes in a tree (training and parsing both enforce it).
pub const MAX_NODES: usize = 1_000_000;

/// A binary split `x[feature] <= threshold` (left) versus `>` (right).
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// Index of the feature tested.
    pub feature: usize,
    /// Rows with `x[feature] <= threshold` go left.
    pub threshold: f64,
    /// Index of the left child; always greater than the node's own index.
    pub left: usize,
    /// Index of the right child; always greater than the node's own index.
    pub right: usize,
}

/// One node of the tree. A node without a [`Split`] is a leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Weighted class counts of the training rows that reached this node (class weights included).
    pub counts: Vec<f64>,
    /// Number of training rows that reached this node.
    pub samples: usize,
    /// The split, or `None` for a leaf.
    pub split: Option<Split>,
}

impl Node {
    /// Total weight of the node (sum of the class counts).
    #[must_use]
    pub fn weight(&self) -> f64 {
        self.counts.iter().sum()
    }
}

/// A trained classification tree. The root is node 0 and every child has a larger index than its
/// parent, so a single ascending pass visits parents before children and a descending pass visits
/// children first.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    pub(crate) feature_names: Vec<String>,
    pub(crate) class_names: Vec<String>,
    pub(crate) config: TrainConfig,
    pub(crate) nodes: Vec<Node>,
}

impl Tree {
    /// Feature names in column order.
    #[must_use]
    pub fn feature_names(&self) -> &[String] {
        &self.feature_names
    }

    /// Class names in label order.
    #[must_use]
    pub fn class_names(&self) -> &[String] {
        &self.class_names
    }

    /// The configuration the tree was trained with (kept for the criterion and as a record).
    #[must_use]
    pub fn config(&self) -> &TrainConfig {
        &self.config
    }

    /// All nodes; index 0 is the root.
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Number of nodes.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of leaves.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.split.is_none()).count()
    }

    /// Depth of the deepest leaf; a tree that is a single leaf has depth 0.
    #[must_use]
    pub fn depth(&self) -> usize {
        let mut depths = vec![0usize; self.nodes.len()];
        let mut deepest = 0;
        for (i, node) in self.nodes.iter().enumerate() {
            let d = depths[i];
            deepest = deepest.max(d);
            if let Some(s) = &node.split {
                for child in [s.left, s.right] {
                    if let Some(slot) = depths.get_mut(child) {
                        *slot = d + 1;
                    }
                }
            }
        }
        deepest
    }

    fn leaf_for(&self, features: &[f64]) -> Result<&Node, CartError> {
        if features.len() != self.feature_names.len() {
            return Err(CartError::FeatureCountMismatch {
                expected: self.feature_names.len(),
                actual: features.len(),
            });
        }
        if let Some(index) = features.iter().position(|v| !v.is_finite()) {
            return Err(CartError::NonFiniteFeature { index });
        }
        let corrupt = || CartError::InvalidModel {
            line: 0,
            reason: "tree structure is inconsistent".to_owned(),
        };
        let mut node = self.nodes.first().ok_or_else(corrupt)?;
        while let Some(split) = &node.split {
            let value = features.get(split.feature).ok_or_else(corrupt)?;
            let next = if *value <= split.threshold {
                split.left
            } else {
                split.right
            };
            node = self.nodes.get(next).ok_or_else(corrupt)?;
        }
        Ok(node)
    }

    /// Predicts the class of one row: the leaf's heaviest class, the lowest label on a tie.
    ///
    /// # Errors
    /// Rejects a wrong feature count and NaN or infinite values.
    pub fn predict(&self, features: &[f64]) -> Result<usize, CartError> {
        let leaf = self.leaf_for(features)?;
        let mut best = 0;
        let mut best_count = f64::NEG_INFINITY;
        for (class, &count) in leaf.counts.iter().enumerate() {
            if count > best_count {
                best = class;
                best_count = count;
            }
        }
        Ok(best)
    }

    /// Class distribution of the leaf the row reaches (weighted counts divided by their total;
    /// all zeros if the leaf weighs nothing). When `class_weights` was set, these are
    /// class-weight-adjusted proportions, not raw frequencies.
    ///
    /// # Errors
    /// Same as [`Tree::predict`].
    pub fn predict_proba(&self, features: &[f64]) -> Result<Vec<f64>, CartError> {
        let leaf = self.leaf_for(features)?;
        let total = leaf.weight();
        if total > 0.0 && total.is_finite() {
            Ok(leaf.counts.iter().map(|&c| c / total).collect())
        } else {
            Ok(vec![0.0; leaf.counts.len()])
        }
    }

    /// Total weighted impurity decrease per feature, normalised to sum to 1 (all zeros when the
    /// tree never splits or every split gained nothing).
    ///
    /// A split at node `t` contributes `W_t i(t) - W_L i(L) - W_R i(R)` using the node's weighted
    /// counts and the tree's criterion; contributions are added in node order.
    #[must_use]
    pub fn feature_importances(&self) -> Vec<f64> {
        let criterion = self.config.criterion;
        let mut importance = vec![0.0; self.feature_names.len()];
        for node in &self.nodes {
            let Some(split) = &node.split else { continue };
            let (Some(left), Some(right)) =
                (self.nodes.get(split.left), self.nodes.get(split.right))
            else {
                continue;
            };
            let gain = node.weight() * criterion.impurity(&node.counts)
                - left.weight() * criterion.impurity(&left.counts)
                - right.weight() * criterion.impurity(&right.counts);
            if let Some(slot) = importance.get_mut(split.feature) {
                *slot += gain.max(0.0);
            }
        }
        let total: f64 = importance.iter().sum();
        if total > 0.0 && total.is_finite() {
            for v in &mut importance {
                *v /= total;
            }
        } else {
            importance.fill(0.0);
        }
        importance
    }
}

/// Structural validation shared by the parser. On failure returns the offending node index and a
/// reason.
pub(crate) fn validate_nodes(
    nodes: &[Node],
    feature_count: usize,
    class_count: usize,
) -> Result<(), (usize, String)> {
    let n = nodes.len();
    if n == 0 || n > MAX_NODES {
        return Err((0, "node count out of range".to_owned()));
    }
    let mut parents = vec![0u8; n];
    for (i, node) in nodes.iter().enumerate() {
        if node.counts.len() != class_count {
            return Err((i, "wrong number of class counts".to_owned()));
        }
        if node.counts.iter().any(|c| !(c.is_finite() && *c >= 0.0)) {
            return Err((i, "class counts must be finite and not negative".to_owned()));
        }
        if !node.weight().is_finite() {
            return Err((i, "class counts overflow".to_owned()));
        }
        if node.samples == 0 {
            return Err((i, "a node needs at least one sample".to_owned()));
        }
        let Some(s) = &node.split else { continue };
        if s.feature >= feature_count {
            return Err((i, "split feature out of range".to_owned()));
        }
        if !s.threshold.is_finite() {
            return Err((i, "split threshold is not finite".to_owned()));
        }
        if s.left <= i || s.right <= i {
            return Err((
                i,
                "child index must be greater than the parent index".to_owned(),
            ));
        }
        if s.left >= n || s.right >= n {
            return Err((i, "child index out of range".to_owned()));
        }
        if s.left == s.right {
            return Err((i, "both children are the same node".to_owned()));
        }
        for child in [s.left, s.right] {
            parents[child] = parents[child].saturating_add(1);
            if parents[child] > 1 {
                return Err((i, "a node has more than one parent".to_owned()));
            }
        }
        let sum = nodes[s.left].samples.checked_add(nodes[s.right].samples);
        if sum != Some(node.samples) {
            return Err((i, "sample counts of the children do not add up".to_owned()));
        }
    }
    if let Some(orphan) = parents.iter().skip(1).position(|&p| p == 0) {
        return Err((orphan + 1, "node is not reachable from the root".to_owned()));
    }
    Ok(())
}
