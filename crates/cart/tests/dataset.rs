//! Dataset validation, stratified splitting and folds.
#![allow(clippy::unwrap_used)]

mod common;

use common::names;
use jobniac_cart::{CartError, Dataset};

fn two_class(per_class: &[usize]) -> Dataset {
    let mut data = Dataset::new(names("f", 2), names("c", per_class.len())).unwrap();
    for (class, &count) in per_class.iter().enumerate() {
        for i in 0..count {
            data.push(&[class as f64, i as f64], class).unwrap();
        }
    }
    data
}

#[test]
fn names_are_validated() {
    let ok = |f: &[&str], c: &[&str]| {
        Dataset::new(
            f.iter().map(|s| (*s).to_owned()).collect(),
            c.iter().map(|s| (*s).to_owned()).collect(),
        )
    };
    assert!(ok(&["a", "B_1", "x.y-z"], &["c0", "c1"]).is_ok());
    assert!(matches!(
        ok(&[""], &["c"]),
        Err(CartError::InvalidName {
            kind: "feature",
            ..
        })
    ));
    assert!(matches!(
        ok(&["a b"], &["c"]),
        Err(CartError::InvalidName { .. })
    ));
    assert!(matches!(
        ok(&["a"], &["c/d"]),
        Err(CartError::InvalidName { kind: "class", .. })
    ));
    assert!(matches!(
        ok(&["ä"], &["c"]),
        Err(CartError::InvalidName { .. })
    ));
    assert!(matches!(
        ok(&["a\n"], &["c"]),
        Err(CartError::InvalidName { .. })
    ));
    assert!(matches!(
        ok(&["a", "a"], &["c"]),
        Err(CartError::DuplicateName {
            kind: "feature",
            ..
        })
    ));
    assert!(matches!(
        ok(&["a"], &["c", "c"]),
        Err(CartError::DuplicateName { kind: "class", .. })
    ));
    assert_eq!(ok(&[], &["c"]).unwrap_err(), CartError::NoFeatures);
    assert_eq!(ok(&["a"], &[]).unwrap_err(), CartError::NoClasses);
    let long = "x".repeat(129);
    assert!(matches!(
        ok(&[&long], &["c"]),
        Err(CartError::InvalidName { .. })
    ));
    let many = names("f", 4097);
    assert!(matches!(
        Dataset::new(many, names("c", 2)),
        Err(CartError::LimitExceeded {
            what: "features",
            ..
        })
    ));
}

#[test]
fn push_rejects_bad_rows_without_storing_them() {
    let mut data = Dataset::new(names("f", 2), names("c", 2)).unwrap();
    assert!(matches!(
        data.push(&[1.0], 0),
        Err(CartError::FeatureCountMismatch {
            expected: 2,
            actual: 1
        })
    ));
    assert!(matches!(
        data.push(&[1.0, f64::NAN], 0),
        Err(CartError::NonFiniteFeature { index: 1 })
    ));
    assert!(matches!(
        data.push(&[f64::INFINITY, 1.0], 0),
        Err(CartError::NonFiniteFeature { index: 0 })
    ));
    assert!(matches!(
        data.push(&[f64::NEG_INFINITY, 1.0], 0),
        Err(CartError::NonFiniteFeature { .. })
    ));
    assert!(matches!(
        data.push(&[1.0, 1.0], 2),
        Err(CartError::LabelOutOfRange {
            label: 2,
            class_count: 2
        })
    ));
    for w in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                data.push_weighted(&[1.0, 1.0], 0, w),
                Err(CartError::InvalidWeight { .. })
            ),
            "{w}"
        );
    }
    assert!(data.is_empty());
    data.push_weighted(&[1.0, 2.0], 1, 2.5).unwrap();
    assert_eq!(
        (data.len(), data.row(0), data.label(0), data.weight(0)),
        (1, &[1.0, 2.0][..], 1, 2.5)
    );
    data.push(&[3.0, 4.0], 0).unwrap();
    assert_eq!(data.weight(1), 1.0);
    assert_eq!(data.labels(), &[1, 0]);
}

#[test]
fn subset_selects_rows_in_order() {
    let data = two_class(&[3, 2]);
    let sub = data.subset(&[4, 0, 0]).unwrap();
    assert_eq!(sub.len(), 3);
    assert_eq!(sub.row(0), data.row(4));
    assert_eq!(sub.labels(), &[1, 0, 0]);
    assert!(matches!(
        data.subset(&[5]),
        Err(CartError::RowOutOfRange { row: 5, len: 5 })
    ));
}

#[test]
fn stratified_split_keeps_class_proportions_and_is_deterministic() {
    let data = two_class(&[20, 10]);
    let (train, test) = data.stratified_split(1, 0.3).unwrap();
    // round(20 * 0.3) = 6 and round(10 * 0.3) = 3 test rows.
    assert_eq!(test.len(), 9);
    assert_eq!(train.len(), 21);
    assert_eq!(test.labels().iter().filter(|&&l| l == 0).count(), 6);
    assert_eq!(test.labels().iter().filter(|&&l| l == 1).count(), 3);
    // Same seed, same split; another seed, another split; rows are never shared.
    assert_eq!(
        data.stratified_split(1, 0.3).unwrap(),
        (train.clone(), test.clone())
    );
    let other = data.stratified_split(2, 0.3).unwrap();
    assert_ne!(other.1, test);
    let key = |d: &Dataset, i: usize| (d.row(i)[0].to_bits(), d.row(i)[1].to_bits());
    let train_keys: Vec<_> = (0..train.len()).map(|i| key(&train, i)).collect();
    assert!((0..test.len()).all(|i| !train_keys.contains(&key(&test, i))));
}

#[test]
fn split_edge_cases() {
    // A single-row class stays in the training set; a two-row class sends exactly one row to test.
    let data = two_class(&[1, 2]);
    let (train, test) = data.stratified_split(0, 0.5).unwrap();
    assert_eq!(test.labels(), &[1]);
    assert_eq!(train.len(), 2);
    // Even a tiny fraction sends one row of a multi-row class to the test set.
    let (_, test) = two_class(&[10, 10]).stratified_split(0, 0.01).unwrap();
    assert_eq!(test.len(), 2);
    for bad in [0.0, 1.0, -0.5, 1.5, f64::NAN] {
        assert!(
            matches!(
                data.stratified_split(0, bad),
                Err(CartError::InvalidArgument { .. })
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        two_class(&[1]).stratified_split(0, 0.5),
        Err(CartError::InvalidArgument { .. })
    ));
}

#[test]
fn stratified_folds_partition_the_rows_evenly() {
    let data = two_class(&[20, 10]);
    let folds = data.stratified_folds(5, 9).unwrap();
    assert_eq!(folds.len(), 5);
    let mut all: Vec<usize> = folds.iter().flatten().copied().collect();
    all.sort_unstable();
    assert_eq!(all, (0..30).collect::<Vec<_>>());
    for fold in &folds {
        assert!(fold.windows(2).all(|w| w[0] < w[1]));
        // 20 / 5 = 4 rows of class 0 and 10 / 5 = 2 of class 1 per fold.
        assert_eq!(fold.iter().filter(|&&r| data.label(r) == 0).count(), 4);
        assert_eq!(fold.iter().filter(|&&r| data.label(r) == 1).count(), 2);
    }
    assert_eq!(folds, data.stratified_folds(5, 9).unwrap());
    assert_ne!(folds, data.stratified_folds(5, 10).unwrap());
    // Sizes differ by at most one when the counts do not divide.
    let sizes: Vec<usize> = two_class(&[7, 5])
        .stratified_folds(5, 0)
        .unwrap()
        .iter()
        .map(Vec::len)
        .collect();
    assert_eq!(sizes.iter().sum::<usize>(), 12);
    assert!(sizes.iter().max().unwrap() - sizes.iter().min().unwrap() <= 1);
    assert!(matches!(
        data.stratified_folds(1, 0),
        Err(CartError::InvalidArgument { .. })
    ));
    assert!(matches!(
        data.stratified_folds(31, 0),
        Err(CartError::InvalidArgument { .. })
    ));
    assert!(data.stratified_folds(30, 0).is_ok());
}
