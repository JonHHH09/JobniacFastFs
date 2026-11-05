//! Cost-complexity pruning against a hand-computed alpha sequence, and alpha selection.
#![allow(clippy::unwrap_used)]

mod common;

use common::one_feature;
use jobniac_cart::{CartError, TrainConfig, cost_complexity_path, prune, select_alpha, train};

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} != {b}");
}

/// x = 1..7 with classes 0,0,1,0,1,1,0 (the tree of the training test).
fn hand_tree() -> jobniac_cart::Tree {
    let labels = [0, 0, 1, 0, 1, 1, 0];
    let rows: Vec<(f64, usize)> = labels
        .iter()
        .enumerate()
        .map(|(i, &y)| ((i + 1) as f64, y))
        .collect();
    train(&one_feature(&rows, 2), &TrainConfig::default()).unwrap()
}

#[test]
fn alpha_sequence_matches_the_hand_computation() {
    // Fully grown Gini tree (all leaves pure, so R(leaf) = 0 and R(T_t) = 0 for every subtree):
    //   A: x <= 2.5   counts [4,3]  R = 24/49  (root, i = 24/49, weight 7/7)
    //   |- leaf [2,0]
    //   `- B: x <= 6.5 counts [2,3]  R = 5/7 * 12/25 = 12/35
    //      |- C: x <= 4.5 counts [1,3]  R = 4/7 * 3/8 = 3/14
    //      |  |- D: x <= 3.5 counts [1,1]  R = 2/7 * 1/2 = 1/7
    //      |  |  |- leaf [0,1]   `- leaf [1,0]
    //      |  `- leaf [0,2]
    //      `- leaf [1,0]
    // Effective alpha = (R(t) - R(T_t)) / (leaves(T_t) - 1):
    //   D = (1/7) / 1 = 1/7          C = (3/14) / 2 = 3/28   <- smallest, collapse C (5 -> 3 leaves)
    //   B = (12/35) / 3 = 4/35       A = (24/49) / 4 = 6/49
    // After C is a leaf with R = 3/14, the subtree of B has 2 leaves and cost 3/14:
    //   B = (12/35 - 3/14) / 1 = 9/70   A = (24/49 - 3/14) / 2 = 27/196   <- B (9/70), 3 -> 2 leaves
    // After B is a leaf with R = 12/35:
    //   A = (24/49 - 12/35) / 1 = 36/245                                  <- collapse to the root
    let tree = hand_tree();
    assert_eq!((tree.node_count(), tree.leaf_count()), (9, 5));
    let path = cost_complexity_path(&tree);
    assert_eq!(path.len(), 4);
    assert_eq!(path[0], 0.0);
    close(path[1], 3.0 / 28.0);
    close(path[2], 9.0 / 70.0);
    close(path[3], 36.0 / 245.0);
    assert!(path.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn pruning_at_each_alpha_gives_the_expected_leaf_counts() {
    let tree = hand_tree();
    let path = cost_complexity_path(&tree);
    let leaves = |alpha: f64| prune(&tree, alpha).unwrap().leaf_count();
    assert_eq!(leaves(0.0), 5);
    assert_eq!(leaves(0.1), 5); // below 3/28 = 0.1071
    assert_eq!(leaves((path[1] + path[2]) / 2.0), 3); // C collapsed
    assert_eq!(leaves(path[1] + 1e-9), 3);
    assert_eq!(leaves((path[2] + path[3]) / 2.0), 2); // B collapsed too
    assert_eq!(leaves(path[3] + 1e-9), 1); // only the root
    assert_eq!(leaves(1e9), 1);

    // The 3-leaf tree is A, leaf [2,0], B (as a leaf [2,3]) with children renumbered 1 and 2, 3, 4.
    let three = prune(&tree, 0.11).unwrap();
    assert_eq!(three.node_count(), 5);
    assert_eq!(three.depth(), 2);
    for (i, node) in three.nodes().iter().enumerate() {
        if let Some(s) = &node.split {
            assert!(s.left > i && s.right > i);
        }
    }
    assert_eq!(three.nodes()[3].counts, vec![1.0, 3.0]); // C became a leaf
    assert_eq!(three.predict(&[3.0]).unwrap(), 1);
    assert_eq!(three.predict(&[4.0]).unwrap(), 1);
    assert_eq!(three.predict(&[7.0]).unwrap(), 0);
    // Pruning is idempotent and never changes the original.
    assert_eq!(prune(&three, 0.11).unwrap(), three);
    assert_eq!(tree.leaf_count(), 5);
    // The root keeps the majority class once everything collapsed.
    let root = prune(&tree, 1.0).unwrap();
    assert_eq!((root.node_count(), root.predict(&[1.0]).unwrap()), (1, 0));
}

#[test]
fn trivial_trees_and_bad_alphas() {
    let data = one_feature(&[(1.0, 0), (2.0, 0)], 2);
    let leaf = train(&data, &TrainConfig::default()).unwrap();
    assert_eq!(cost_complexity_path(&leaf), vec![0.0]);
    assert_eq!(prune(&leaf, 0.5).unwrap(), leaf);
    for bad in [-0.1, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(prune(&leaf, bad), Err(CartError::InvalidArgument { .. })),
            "{bad}"
        );
    }
}

#[test]
fn zero_gain_splits_go_first_at_alpha_zero() {
    // XOR: the root split gains nothing, so its subtree is exactly as costly as the root.
    let mut data =
        jobniac_cart::Dataset::new(common::names("f", 2), common::names("c", 2)).unwrap();
    for (a, b) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)] {
        data.push(&[a, b], usize::from(a != b)).unwrap();
    }
    let tree = train(&data, &TrainConfig::default()).unwrap();
    assert_eq!(tree.leaf_count(), 4);
    // Root: counts [2,2], i = 1/2, R = 1/2. Its two children are [1,1] nodes with R = 2/4 * 1/2
    // = 1/4 each. All four leaves are pure. Alphas: root (1/2 - 0) / 3 = 1/6, each child
    // (1/4 - 0) / 1 = 1/4. The root is the weakest link and collapsing it removes everything.
    let path = cost_complexity_path(&tree);
    assert_eq!(path.len(), 2);
    close(path[1], 1.0 / 6.0);
    assert_eq!(
        prune(&tree, *path.last().unwrap() + 1.0)
            .unwrap()
            .leaf_count(),
        1
    );
}

#[test]
fn select_alpha_is_deterministic_and_consistent() {
    let data = common::synthetic(21, 240);
    let cfg = TrainConfig::default();
    let a = select_alpha(&data, &cfg, 5, 99).unwrap();
    let b = select_alpha(&data, &cfg, 5, 99).unwrap();
    assert_eq!(a, b);

    let full = train(&data, &cfg).unwrap();
    let path = cost_complexity_path(&full);
    assert_eq!(a.scores.len(), path.len());
    for (score, alpha) in a.scores.iter().zip(&path) {
        assert_eq!(score.alpha, *alpha);
        assert!((0.0..=1.0).contains(&score.mean_accuracy));
    }
    // The winner has the best score, and no larger alpha is as good (ties go to the larger alpha).
    let best = a.scores[a.best_index].mean_accuracy;
    assert!(a.scores.iter().all(|s| s.mean_accuracy <= best));
    assert!(
        a.scores[a.best_index + 1..]
            .iter()
            .all(|s| s.mean_accuracy < best)
    );
    assert_eq!(a.best_alpha, a.scores[a.best_index].alpha);
    // The overfitted full tree is not the best choice on noisy data.
    assert!(a.best_index > 0);
    // A different seed may pick different folds, but still works.
    assert!(select_alpha(&data, &cfg, 3, 1).is_ok());
}

#[test]
fn select_alpha_prefers_the_simpler_tree_on_equal_scores() {
    // Separable data with a wide gap between the classes: every training fold places its single
    // threshold inside the gap, so alpha 0 scores 100% and collapsing the root cannot.
    let rows: Vec<(f64, usize)> = (0..40)
        .map(|i| {
            (
                f64::from(i) + if i >= 20 { 100.0 } else { 0.0 },
                usize::from(i >= 20),
            )
        })
        .collect();
    let data = one_feature(&rows, 2);
    let sel = select_alpha(&data, &TrainConfig::default(), 4, 3).unwrap();
    assert_eq!(sel.scores.len(), 2);
    assert_eq!(sel.scores[0].mean_accuracy, 1.0);
    assert!(sel.scores[1].mean_accuracy < 1.0);
    assert_eq!(sel.best_index, 0);
}

#[test]
fn select_alpha_survives_score_numerators_that_overflow_u128() {
    // 20 folds of 80 rows: the common denominator 80^20 fits in u128, but the scaled hit counts
    // (up to 80 * 80^19 per fold, summed over 20 folds) do not. The comparison must fall back to
    // the float means instead of overflowing.
    let rows: Vec<(f64, usize)> = (0..1600)
        .map(|i| (f64::from(i % 2), (i % 2) as usize))
        .collect();
    let data = one_feature(&rows, 2);
    let first = select_alpha(&data, &TrainConfig::default(), 20, 0).unwrap();
    let again = select_alpha(&data, &TrainConfig::default(), 20, 0).unwrap();
    assert_eq!(first.best_index, again.best_index);
    assert_eq!(first.best_alpha.to_bits(), again.best_alpha.to_bits());
    // The data is separable by the single split, so the unpruned tree is perfect on every fold.
    assert_eq!(first.scores[0].mean_accuracy, 1.0);
    assert_eq!(first.best_index, 0);
}

#[test]
fn select_alpha_rejects_bad_arguments() {
    let data = common::synthetic(1, 10);
    let cfg = TrainConfig::default();
    assert!(matches!(
        select_alpha(&data, &cfg, 1, 0),
        Err(CartError::InvalidArgument { .. })
    ));
    assert!(matches!(
        select_alpha(&data, &cfg, 11, 0),
        Err(CartError::InvalidArgument { .. })
    ));
    let bad = TrainConfig {
        min_samples_leaf: 0,
        ..TrainConfig::default()
    };
    assert!(matches!(
        select_alpha(&data, &bad, 2, 0),
        Err(CartError::InvalidConfig { .. })
    ));
}
