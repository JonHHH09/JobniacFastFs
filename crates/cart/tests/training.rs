//! Training: hand-computed splits, tie-breaking, stopping rules, validation.
#![allow(clippy::unwrap_used)]

mod common;

use common::{names, one_feature};
use jobniac_cart::{CartError, Criterion, Dataset, SplitMix64, TrainConfig, train};

/// x = 1..7 with classes 0,0,1,0,1,1,0.
fn hand_dataset() -> Dataset {
    let labels = [0, 0, 1, 0, 1, 1, 0];
    let rows: Vec<(f64, usize)> = labels
        .iter()
        .enumerate()
        .map(|(i, &y)| ((i + 1) as f64, y))
        .collect();
    one_feature(&rows, 2)
}

#[test]
fn best_root_split_matches_the_hand_computation() {
    // Root counts [4, 3], Gini = 1 - (16 + 9) / 49 = 24/49.
    //   x <= 1.5: [1,0] | [3,3] -> 6/7 * 1/2 = 3/7                     gain 24/49 - 21/49 = 3/49
    //   x <= 2.5: [2,0] | [2,3] -> 5/7 * 12/25 = 12/35                 gain 36/245  (best)
    //   x <= 3.5: [2,1] | [2,2] -> 3/7 * 4/9 + 4/7 * 1/2 = 10/21      gain 24/49 - 10/21 = 2/147
    //   x <= 4.5: [3,1] | [1,2] -> 4/7 * 3/8 + 3/7 * 4/9 = 17/42      gain 25/294
    //   x <= 5.5: [3,2] | [1,1] -> 5/7 * 12/25 + 2/7 * 1/2 = 17/35    gain 1/245
    //   x <= 6.5: [3,3] | [1,0] -> 6/7 * 1/2 = 3/7                     gain 3/49
    // The maximum is 36/245 at the midpoint of 2 and 3.
    let tree = train(&hand_dataset(), &TrainConfig::default()).unwrap();
    let root = &tree.nodes()[0];
    let split = root.split.as_ref().unwrap();
    assert_eq!(split.feature, 0);
    assert_eq!(split.threshold, 2.5);
    assert_eq!(root.counts, vec![4.0, 3.0]);
    assert_eq!(root.samples, 7);
    let left = &tree.nodes()[split.left];
    let right = &tree.nodes()[split.right];
    assert_eq!((left.counts.clone(), left.samples), (vec![2.0, 0.0], 2));
    assert_eq!((right.counts.clone(), right.samples), (vec![2.0, 3.0], 5));
    assert!(left.split.is_none());
    // Unlimited depth separates everything: 5 leaves, as computed in the pruning test.
    assert_eq!(tree.leaf_count(), 5);
    assert_eq!(tree.node_count(), 9);
    assert_eq!(tree.depth(), 4);
}

#[test]
fn best_split_ignores_an_uninformative_feature() {
    let mut data = Dataset::new(names("f", 2), names("c", 2)).unwrap();
    for (i, y) in [0, 0, 1, 0, 1, 1, 0].into_iter().enumerate() {
        data.push(&[1.0, (i + 1) as f64], y).unwrap(); // feature 0 is constant
    }
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let split = tree.nodes()[0].split.as_ref().unwrap();
    assert_eq!((split.feature, split.threshold), (1, 2.5));
    assert_eq!(tree.feature_importances()[0], 0.0);
    assert!((tree.feature_importances()[1] - 1.0).abs() < 1e-15);
}

#[test]
fn identical_features_tie_to_the_lower_index() {
    let mut data = Dataset::new(names("f", 3), names("c", 2)).unwrap();
    for (i, y) in [0, 0, 1, 0, 1, 1, 0].into_iter().enumerate() {
        let x = (i + 1) as f64;
        data.push(&[x, x, x], y).unwrap();
    }
    let cfg = TrainConfig {
        max_depth: Some(1),
        ..TrainConfig::default()
    };
    let tree = train(&data, &cfg).unwrap();
    assert_eq!(tree.nodes()[0].split.as_ref().unwrap().feature, 0);
    // Swapping which column holds the informative values moves the winner accordingly.
    let mut data = Dataset::new(names("f", 3), names("c", 2)).unwrap();
    for (i, y) in [0, 0, 1, 0, 1, 1, 0].into_iter().enumerate() {
        let x = (i + 1) as f64;
        data.push(&[0.0, x, x], y).unwrap();
    }
    let tree = train(&data, &cfg).unwrap();
    assert_eq!(tree.nodes()[0].split.as_ref().unwrap().feature, 1);
}

#[test]
fn equal_gain_thresholds_tie_to_the_lower_threshold() {
    // x = 1..4 with classes 0,1,0,1: x <= 1.5 gives [1,0] | [1,2] and x <= 3.5 gives [2,1] | [0,1];
    // both lose 1/2 - 3/4 * 4/9 = 1/6, and x <= 2.5 gains nothing. The lower threshold wins.
    let data = one_feature(&[(1.0, 0), (2.0, 1), (3.0, 0), (4.0, 1)], 2);
    let cfg = TrainConfig {
        max_depth: Some(1),
        ..TrainConfig::default()
    };
    let tree = train(&data, &cfg).unwrap();
    assert_eq!(tree.nodes()[0].split.as_ref().unwrap().threshold, 1.5);
}

#[test]
fn thresholds_use_the_overflow_safe_midpoint() {
    let data = one_feature(&[(-1.0e308, 0), (1.0e308, 1)], 2);
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let t = tree.nodes()[0].split.as_ref().unwrap().threshold;
    assert!(t.is_finite() && (-1.0e308..1.0e308).contains(&t));
    assert_eq!(tree.predict(&[-1.0e308]).unwrap(), 0);
    assert_eq!(tree.predict(&[1.0e308]).unwrap(), 1);
    // Adjacent doubles: the midpoint rounds to the upper value, so the lower one is used.
    let a = 1.0_f64;
    let b = f64::from_bits(a.to_bits() + 1);
    let data = one_feature(&[(a, 0), (b, 1)], 2);
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let t = tree.nodes()[0].split.as_ref().unwrap().threshold;
    assert_eq!(t, a);
    assert_eq!(tree.predict(&[a]).unwrap(), 0);
    assert_eq!(tree.predict(&[b]).unwrap(), 1);
}

#[test]
fn negative_and_positive_zero_are_not_separated() {
    let data = one_feature(&[(-0.0, 0), (0.0, 1), (1.0, 1)], 2);
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let t = tree.nodes()[0].split.as_ref().unwrap().threshold;
    assert_eq!(t, 0.5);
    assert_eq!(tree.nodes()[0].samples, 3);
}

#[test]
fn single_class_data_gives_one_leaf() {
    let data = one_feature(&[(1.0, 1), (2.0, 1), (3.0, 1)], 3);
    let tree = train(&data, &TrainConfig::default()).unwrap();
    assert_eq!(tree.node_count(), 1);
    assert_eq!(tree.leaf_count(), 1);
    assert_eq!(tree.depth(), 0);
    assert_eq!(tree.predict(&[10.0]).unwrap(), 1);
    assert_eq!(tree.predict_proba(&[10.0]).unwrap(), vec![0.0, 1.0, 0.0]);
    assert_eq!(tree.feature_importances(), vec![0.0]);
}

#[test]
fn xor_needs_a_zero_gain_split_and_is_learned() {
    let mut data = Dataset::new(names("f", 2), names("c", 2)).unwrap();
    for (a, b) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)] {
        data.push(&[a, b], usize::from(a != b)).unwrap();
    }
    for criterion in [Criterion::Gini, Criterion::Entropy] {
        let cfg = TrainConfig {
            criterion,
            ..TrainConfig::default()
        };
        let tree = train(&data, &cfg).unwrap();
        for i in 0..data.len() {
            assert_eq!(tree.predict(data.row(i)).unwrap(), data.label(i));
        }
        assert_eq!(tree.leaf_count(), 4);
    }
}

#[test]
fn separable_data_reaches_perfect_training_accuracy() {
    // 300 distinct points with random labels: only a fully grown tree can fit them.
    let mut rng = SplitMix64::new(7);
    let mut data = Dataset::new(names("f", 4), names("c", 3)).unwrap();
    for _ in 0..300 {
        let row = [
            rng.next_f64(),
            rng.next_f64(),
            rng.next_f64(),
            rng.next_f64(),
        ];
        data.push(&row, rng.next_below(3)).unwrap();
    }
    for criterion in [Criterion::Gini, Criterion::Entropy] {
        let cfg = TrainConfig {
            criterion,
            ..TrainConfig::default()
        };
        let tree = train(&data, &cfg).unwrap();
        let correct = (0..data.len())
            .filter(|&i| tree.predict(data.row(i)).unwrap() == data.label(i))
            .count();
        assert_eq!(correct, data.len());
    }
}

#[test]
fn nodes_keep_the_structural_invariants() {
    let data = common::synthetic(11, 200);
    let tree = train(&data, &TrainConfig::default()).unwrap();
    for (i, node) in tree.nodes().iter().enumerate() {
        if let Some(s) = &node.split {
            assert!(s.left > i && s.right > i);
            let (l, r) = (&tree.nodes()[s.left], &tree.nodes()[s.right]);
            assert_eq!(l.samples + r.samples, node.samples);
            for k in 0..node.counts.len() {
                assert!((l.counts[k] + r.counts[k] - node.counts[k]).abs() < 1e-9);
            }
        }
    }
    let probs = tree.predict_proba(data.row(0)).unwrap();
    assert!((probs.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    let importances = tree.feature_importances();
    assert!((importances.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(importances.iter().all(|&v| (0.0..=1.0).contains(&v)));
}

#[test]
fn stopping_rules_are_respected() {
    let data = common::synthetic(5, 300);

    let stump = train(
        &data,
        &TrainConfig {
            max_depth: Some(1),
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert_eq!((stump.depth(), stump.leaf_count()), (1, 2));
    let root_only = train(
        &data,
        &TrainConfig {
            max_depth: Some(0),
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert_eq!(root_only.node_count(), 1);

    let leafy = train(
        &data,
        &TrainConfig {
            min_samples_leaf: 20,
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert!(
        leafy
            .nodes()
            .iter()
            .filter(|n| n.split.is_none())
            .all(|n| n.samples >= 20)
    );

    let splitty = train(
        &data,
        &TrainConfig {
            min_samples_split: 60,
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert!(
        splitty
            .nodes()
            .iter()
            .filter(|n| n.split.is_some())
            .all(|n| n.samples >= 60)
    );

    // The root's gain is well below 1, so a huge threshold keeps a single leaf.
    let strict = train(
        &data,
        &TrainConfig {
            min_impurity_decrease: 0.9,
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert_eq!(strict.node_count(), 1);
    // Every kept split respects the weighted decrease bound.
    let cfg = TrainConfig {
        min_impurity_decrease: 0.01,
        ..TrainConfig::default()
    };
    let tree = train(&data, &cfg).unwrap();
    let root_weight = tree.nodes()[0].weight();
    for node in tree.nodes() {
        if let Some(s) = &node.split {
            let (l, r) = (&tree.nodes()[s.left], &tree.nodes()[s.right]);
            let gain = Criterion::Gini.impurity(&node.counts)
                - l.weight() / node.weight() * Criterion::Gini.impurity(&l.counts)
                - r.weight() / node.weight() * Criterion::Gini.impurity(&r.counts);
            assert!(node.weight() / root_weight * gain >= 0.01 - 1e-12);
        }
    }
}

#[test]
fn row_and_class_weights_scale_the_counts() {
    let mut data = Dataset::new(names("x", 1), names("c", 2)).unwrap();
    data.push_weighted(&[1.0], 0, 2.0).unwrap();
    data.push_weighted(&[2.0], 1, 3.0).unwrap();
    data.push(&[3.0], 1).unwrap();
    let tree = train(
        &data,
        &TrainConfig {
            max_depth: Some(0),
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert_eq!(tree.nodes()[0].counts, vec![2.0, 4.0]);
    assert_eq!(tree.nodes()[0].samples, 3);

    let cfg = TrainConfig {
        max_depth: Some(0),
        class_weights: Some(vec![5.0, 0.5]),
        ..TrainConfig::default()
    };
    let tree = train(&data, &cfg).unwrap();
    assert_eq!(tree.nodes()[0].counts, vec![10.0, 2.0]);
    assert_eq!(tree.predict(&[1.0]).unwrap(), 0);
    let unweighted = train(
        &data,
        &TrainConfig {
            max_depth: Some(0),
            ..TrainConfig::default()
        },
    )
    .unwrap();
    assert_eq!(unweighted.predict(&[1.0]).unwrap(), 1);
}

#[test]
fn training_is_repeatable() {
    let data = common::synthetic(3, 250);
    let a = train(&data, &TrainConfig::default()).unwrap();
    let b = train(&data, &TrainConfig::default()).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.to_text(), b.to_text());
}

#[test]
fn config_validation_errors() {
    let data = one_feature(&[(1.0, 0), (2.0, 1)], 2);
    let bad = [
        TrainConfig {
            min_samples_split: 1,
            ..TrainConfig::default()
        },
        TrainConfig {
            min_samples_leaf: 0,
            ..TrainConfig::default()
        },
        TrainConfig {
            min_impurity_decrease: -0.1,
            ..TrainConfig::default()
        },
        TrainConfig {
            min_impurity_decrease: f64::NAN,
            ..TrainConfig::default()
        },
        TrainConfig {
            min_impurity_decrease: f64::INFINITY,
            ..TrainConfig::default()
        },
        TrainConfig {
            class_weights: Some(vec![]),
            ..TrainConfig::default()
        },
        TrainConfig {
            class_weights: Some(vec![1.0, 0.0]),
            ..TrainConfig::default()
        },
        TrainConfig {
            class_weights: Some(vec![1.0, f64::NAN]),
            ..TrainConfig::default()
        },
        TrainConfig {
            class_weights: Some(vec![1.0, -2.0]),
            ..TrainConfig::default()
        },
    ];
    for cfg in &bad {
        assert!(
            matches!(cfg.validate(), Err(CartError::InvalidConfig { .. })),
            "{cfg:?}"
        );
        assert!(matches!(
            train(&data, cfg),
            Err(CartError::InvalidConfig { .. })
        ));
    }
    // Wrong class-weight length is only detectable against a dataset.
    let cfg = TrainConfig {
        class_weights: Some(vec![1.0, 1.0, 1.0]),
        ..TrainConfig::default()
    };
    assert!(cfg.validate().is_ok());
    assert!(matches!(
        train(&data, &cfg),
        Err(CartError::InvalidConfig { .. })
    ));
    assert!(TrainConfig::default().validate().is_ok());
}

#[test]
fn empty_dataset_is_rejected() {
    let data = Dataset::new(names("x", 1), names("c", 2)).unwrap();
    assert_eq!(
        train(&data, &TrainConfig::default()),
        Err(CartError::EmptyDataset)
    );
}

#[test]
fn predict_validates_its_input() {
    let tree = train(&hand_dataset(), &TrainConfig::default()).unwrap();
    assert!(matches!(
        tree.predict(&[]),
        Err(CartError::FeatureCountMismatch {
            expected: 1,
            actual: 0
        })
    ));
    assert!(matches!(
        tree.predict(&[1.0, 2.0]),
        Err(CartError::FeatureCountMismatch { .. })
    ));
    assert!(matches!(
        tree.predict(&[f64::NAN]),
        Err(CartError::NonFiniteFeature { index: 0 })
    ));
    assert!(matches!(
        tree.predict_proba(&[f64::INFINITY]),
        Err(CartError::NonFiniteFeature { .. })
    ));
    // `x <= threshold` goes left: exactly the threshold is on the left.
    assert_eq!(tree.predict(&[2.5]).unwrap(), 0);
    assert_eq!(tree.predict(&[2.6]).unwrap(), 1);
}

#[test]
fn overflowing_weights_are_rejected() {
    // 1e308 * 1e10 overflows to infinity.
    let mut data = Dataset::new(names("x", 1), names("c", 2)).unwrap();
    data.push_weighted(&[1.0], 0, 1.0e308).unwrap();
    data.push(&[2.0], 1).unwrap();
    let cfg = TrainConfig {
        class_weights: Some(vec![1.0e10, 1.0]),
        ..TrainConfig::default()
    };
    assert!(matches!(
        train(&data, &cfg),
        Err(CartError::InvalidWeight { .. })
    ));
    // Each product is finite but the total is not.
    let mut data = Dataset::new(names("x", 1), names("c", 2)).unwrap();
    data.push_weighted(&[1.0], 0, 1.0e308).unwrap();
    data.push_weighted(&[2.0], 1, 1.0e308).unwrap();
    data.push_weighted(&[3.0], 1, 1.0e308).unwrap();
    assert!(matches!(
        train(&data, &TrainConfig::default()),
        Err(CartError::InvalidWeight { .. })
    ));
}

#[test]
fn row_access_does_not_wrap_on_huge_indices() {
    let data = hand_dataset();
    assert!(data.row(1 << 63).is_empty());
    assert!(data.row(usize::MAX).is_empty());
    assert!(data.row(7).is_empty());
    assert_eq!(data.row(6), &[7.0]);
}
