//! Greedy CART training.
//!
//! Complexity: at every node each feature costs one sort, `O(n_t log n_t)`, plus a linear sweep of
//! `O(n_t * K)` for `K` classes, so a level of the tree costs `O(F * n * (log n + K))` and a tree of
//! depth `D` costs about `O(D * F * n * log n)`. Memory is `O(n + nodes * K)`.

use crate::config::{Criterion, TrainConfig};
use crate::dataset::Dataset;
use crate::error::CartError;
use crate::tree::{MAX_NODES, Node, Split, Tree};

/// The best split found for a node.
struct Candidate {
    feature: usize,
    threshold: f64,
    /// Unweighted-by-node-fraction gain `i(t) - wl/W * i(L) - wr/W * i(R)`, never negative.
    gain: f64,
}

/// Midpoint of two distinct, ordered values `a < b`, falling back to `a` when rounding (or an
/// overflowing difference) would not leave it strictly below `b`.
pub(crate) fn midpoint(a: f64, b: f64) -> f64 {
    let t = a + (b - a) / 2.0;
    if t.is_finite() && t < b { t } else { a }
}

/// Weighted class counts of `rows`, summed in the order given.
fn count_classes(data: &Dataset, weights: &[f64], rows: &[usize]) -> Vec<f64> {
    let mut counts = vec![0.0; data.class_count()];
    for &row in rows {
        if let Some(slot) = counts.get_mut(data.label(row)) {
            *slot += weights.get(row).copied().unwrap_or(0.0);
        }
    }
    counts
}

/// Best split of a node by exhaustive sweep, or `None` when no admissible threshold exists.
///
/// Features are visited in ascending order and thresholds ascending within a feature, and a
/// candidate replaces the incumbent only when its gain is strictly larger, so exact ties go to the
/// lower feature index and then the lower threshold.
fn best_split(
    data: &Dataset,
    weights: &[f64],
    rows: &[usize],
    node_counts: &[f64],
    config: &TrainConfig,
) -> Option<Candidate> {
    let criterion: Criterion = config.criterion;
    let n = rows.len();
    let total: f64 = node_counts.iter().sum();
    if n < 2 || total <= 0.0 {
        return None;
    }
    let node_impurity = criterion.impurity(node_counts);
    let min_leaf = config.min_samples_leaf;
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut left = vec![0.0; node_counts.len()];
    let mut right = vec![0.0; node_counts.len()];
    let mut best: Option<Candidate> = None;

    for feature in 0..data.feature_count() {
        order.clear();
        order.extend_from_slice(rows);
        order.sort_unstable_by(|&a, &b| {
            data.value(a, feature)
                .total_cmp(&data.value(b, feature))
                .then(a.cmp(&b))
        });
        left.fill(0.0);
        for pos in 0..n - 1 {
            let row = order[pos];
            if let Some(slot) = left.get_mut(data.label(row)) {
                *slot += weights.get(row).copied().unwrap_or(0.0);
            }
            let left_n = pos + 1;
            if left_n < min_leaf || n - left_n < min_leaf {
                continue;
            }
            let a = data.value(row, feature);
            let b = data.value(order[pos + 1], feature);
            if a >= b {
                continue; // equal values (including -0.0 and 0.0) cannot be separated
            }
            for ((r, &t), &l) in right.iter_mut().zip(node_counts).zip(&left) {
                *r = (t - l).max(0.0);
            }
            let wl: f64 = left.iter().sum();
            let wr: f64 = right.iter().sum();
            let gain = (node_impurity
                - (wl / total) * criterion.impurity(&left)
                - (wr / total) * criterion.impurity(&right))
            .max(0.0);
            if best.as_ref().is_none_or(|incumbent| gain > incumbent.gain) {
                best = Some(Candidate {
                    feature,
                    threshold: midpoint(a, b),
                    gain,
                });
            }
        }
    }
    best
}

/// Trains a classification tree.
///
/// Greedy recursive binary splitting on `x[feature] <= threshold`. Each node and feature is swept
/// over the rows sorted by `(value, row index)` with cumulative weighted class counts; only
/// thresholds between distinct consecutive values that leave at least `min_samples_leaf` rows on
/// both sides are considered, the threshold being the midpoint `a + (b - a) / 2` (or `a` if
/// rounding would make it equal `b`). The split with the largest weighted impurity decrease wins;
/// exact ties go to the lower feature index, then the lower threshold. A node becomes a leaf when
/// it is at `max_depth`, has fewer than `min_samples_split` rows, is pure, has no admissible split,
/// or the best split's `W_t / W_root * gain` is below `min_impurity_decrease`.
///
/// Exact ties are ties between bit-equal gains; two mathematically equal gains that round to
/// different bits are ordered by the larger float. This is deterministic either way and no epsilon
/// is used.
///
/// Row weights are the dataset weight times the class weight. The result depends only on the
/// inputs: no threads, no hash maps, fixed summation order. (Entropy uses a portable `log2`, so
/// results do not depend on the platform's math library either.)
///
/// # Errors
/// An invalid config, an empty dataset, class weights whose length differs from the class count, or
/// a tree that would exceed [`MAX_NODES`].
pub fn train(data: &Dataset, config: &TrainConfig) -> Result<Tree, CartError> {
    config.validate()?;
    if data.is_empty() {
        return Err(CartError::EmptyDataset);
    }
    if let Some(cw) = &config.class_weights
        && cw.len() != data.class_count()
    {
        return Err(CartError::InvalidConfig {
            reason: format!(
                "class_weights has {} entries for {} classes",
                cw.len(),
                data.class_count()
            ),
        });
    }
    let weights: Vec<f64> = (0..data.len())
        .map(|i| {
            let class_weight = config
                .class_weights
                .as_ref()
                .and_then(|cw| cw.get(data.label(i)))
                .copied()
                .unwrap_or(1.0);
            data.weight(i) * class_weight
        })
        .collect();

    if let Some(&weight) = weights.iter().find(|w| !(w.is_finite() && **w > 0.0)) {
        return Err(CartError::InvalidWeight { weight });
    }
    let total_weight: f64 = weights.iter().sum();
    if !total_weight.is_finite() {
        return Err(CartError::InvalidWeight {
            weight: total_weight,
        });
    }
    let all_rows: Vec<usize> = (0..data.len()).collect();
    let root_counts = count_classes(data, &weights, &all_rows);
    let root_weight: f64 = root_counts.iter().sum();
    let mut nodes = vec![Node {
        counts: root_counts,
        samples: data.len(),
        split: None,
    }];
    // Work list of (node index, rows, depth). Children are appended to `nodes` as soon as their
    // parent splits, which guarantees child index > parent index.
    let mut stack = vec![(0usize, all_rows, 0usize)];
    while let Some((id, rows, depth)) = stack.pop() {
        let counts = nodes[id].counts.clone();
        let pure = counts.iter().filter(|&&c| c > 0.0).count() <= 1;
        if pure
            || rows.len() < config.min_samples_split
            || config.max_depth.is_some_and(|max| depth >= max)
        {
            continue;
        }
        let Some(best) = best_split(data, &weights, &rows, &counts, config) else {
            continue;
        };
        let node_weight: f64 = counts.iter().sum();
        let decrease = (node_weight / root_weight) * best.gain;
        if decrease < config.min_impurity_decrease {
            continue;
        }
        let (left_rows, right_rows): (Vec<usize>, Vec<usize>) = rows
            .iter()
            .partition(|&&row| data.value(row, best.feature) <= best.threshold);
        if left_rows.is_empty() || right_rows.is_empty() {
            continue;
        }
        if nodes.len() + 2 > MAX_NODES {
            return Err(CartError::LimitExceeded {
                what: "nodes",
                max: MAX_NODES,
            });
        }
        let left_id = nodes.len();
        let right_id = left_id + 1;
        nodes.push(Node {
            counts: count_classes(data, &weights, &left_rows),
            samples: left_rows.len(),
            split: None,
        });
        nodes.push(Node {
            counts: count_classes(data, &weights, &right_rows),
            samples: right_rows.len(),
            split: None,
        });
        nodes[id].split = Some(Split {
            feature: best.feature,
            threshold: best.threshold,
            left: left_id,
            right: right_id,
        });
        stack.push((right_id, right_rows, depth + 1));
        stack.push((left_id, left_rows, depth + 1));
    }

    Ok(Tree {
        feature_names: data.feature_names().to_vec(),
        class_names: data.class_names().to_vec(),
        config: config.clone(),
        nodes,
    })
}
