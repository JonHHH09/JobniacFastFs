//! Choosing the pruning strength by stratified cross-validation.

use crate::config::TrainConfig;
use crate::dataset::Dataset;
use crate::error::CartError;
use crate::prune::{cost_complexity_path, prune_links, weakest_links};
use crate::train::train;

/// Mean held-out accuracy of one candidate alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlphaScore {
    /// The candidate alpha.
    pub alpha: f64,
    /// Mean over the folds of (correct predictions / rows) on the held-out fold, unweighted.
    pub mean_accuracy: f64,
}

/// Result of [`select_alpha`].
#[derive(Debug, Clone, PartialEq)]
pub struct AlphaSelection {
    /// One score per candidate alpha, in increasing alpha order.
    pub scores: Vec<AlphaScore>,
    /// Index into `scores` of the chosen alpha.
    pub best_index: usize,
    /// The chosen alpha: the best mean accuracy, the larger alpha on a tie.
    pub best_alpha: f64,
}

/// Picks a cost-complexity alpha by `folds`-fold stratified cross-validation.
///
/// Candidates are [`cost_complexity_path`] of a tree grown on the whole dataset. For each fold a
/// tree is grown on the other `folds - 1` folds, pruned at every candidate and scored on the
/// held-out fold; scores are averaged over the folds (each fold counts equally). The best mean
/// accuracy wins and ties go to the larger alpha, i.e. the simpler tree. Folds come from
/// [`Dataset::stratified_folds`], so the result is a pure function of the arguments. To get the
/// final model, `train` on all data and `prune` at `best_alpha`.
///
/// # Errors
/// Anything [`train`], [`Dataset::stratified_folds`] or [`crate::prune`] reports, for example fewer rows
/// than folds.
pub fn select_alpha(
    dataset: &Dataset,
    config: &TrainConfig,
    folds: usize,
    seed: u64,
) -> Result<AlphaSelection, CartError> {
    let fold_rows = dataset.stratified_folds(folds, seed)?;
    let full = train(dataset, config)?;
    let alphas = cost_complexity_path(&full);
    // Exact integer tallies: correct predictions and held-out rows per (alpha, fold).
    let mut correct = vec![vec![0usize; fold_rows.len()]; alphas.len()];
    for (f, held_out) in fold_rows.iter().enumerate() {
        let train_rows: Vec<usize> = (0..dataset.len())
            .filter(|row| held_out.binary_search(row).is_err())
            .collect();
        let tree = train(&dataset.subset(&train_rows)?, config)?;
        // One weakest-link sequence per fold tree; each alpha takes the prefix with alpha_i <= alpha.
        let links = weakest_links(&tree, f64::INFINITY);
        let mut cache: Option<(usize, usize)> = None; // (prefix length, correct)
        for (a, &alpha) in alphas.iter().enumerate() {
            let k = links.partition_point(|l| l.alpha <= alpha);
            let hits = match cache {
                Some((ck, hits)) if ck == k => hits,
                _ => {
                    let pruned = prune_links(&tree, &links[..k]);
                    let mut hits = 0usize;
                    for &row in held_out {
                        if pruned.predict(dataset.row(row))? == dataset.label(row) {
                            hits += 1;
                        }
                    }
                    cache = Some((k, hits));
                    hits
                }
            };
            correct[a][f] = hits;
        }
    }
    let scores: Vec<AlphaScore> = alphas
        .iter()
        .zip(&correct)
        .map(|(&alpha, hits)| {
            let total: f64 = hits
                .iter()
                .zip(&fold_rows)
                .map(|(&h, fold)| h as f64 / fold.len() as f64)
                .sum();
            AlphaScore {
                alpha,
                mean_accuracy: total / folds as f64,
            }
        })
        .collect();
    // Compare candidates by exact fractions: the mean accuracy is sum_f h_f / n_f over folds, so
    // two candidates tie exactly when every fold has the same correct count.
    let mut best_index = 0;
    for i in 1..alphas.len() {
        if !worse(&correct[i], &correct[best_index], &fold_rows) {
            best_index = i;
        }
    }
    Ok(AlphaSelection {
        best_alpha: scores[best_index].alpha,
        best_index,
        scores,
    })
}

/// True when candidate `a` has a strictly lower mean accuracy than `b`.
///
/// Means over folds are `sum_f a_f / n_f`; they are compared with exact rational arithmetic
/// (cross-multiplied in `u128`), so ties are exact and independent of float rounding. When the
/// common denominator or either scaled numerator does not fit in `u128`, the float sums are
/// compared instead (still deterministic). Empty folds contribute nothing.
fn worse(a: &[usize], b: &[usize], folds: &[Vec<usize>]) -> bool {
    match (exact_numerator(a, folds), exact_numerator(b, folds)) {
        (Some(left), Some(right)) => left < right,
        _ => float_mean(a, folds) < float_mean(b, folds),
    }
}

/// `sum_f hits_f * (prod(n) / n_f)` over the non-empty folds, or `None` when the common
/// denominator `prod(n)` or any partial sum overflows `u128`.
fn exact_numerator(hits: &[usize], folds: &[Vec<usize>]) -> Option<u128> {
    let den = folds
        .iter()
        .filter(|fold| !fold.is_empty())
        .try_fold(1u128, |den, fold| den.checked_mul(fold.len() as u128))?;
    hits.iter()
        .zip(folds)
        .filter(|(_, fold)| !fold.is_empty())
        .try_fold(0u128, |sum, (&x, fold)| {
            let scaled = (x as u128).checked_mul(den / fold.len() as u128)?;
            sum.checked_add(scaled)
        })
}

/// `sum_f hits_f / n_f` over the non-empty folds, in fold order.
fn float_mean(hits: &[usize], folds: &[Vec<usize>]) -> f64 {
    hits.iter()
        .zip(folds)
        .filter(|(_, fold)| !fold.is_empty())
        .map(|(&x, fold)| x as f64 / fold.len() as f64)
        .sum()
}
