//! Impurity measures against hand-computed values, and the SplitMix64 reference sequence.
#![allow(clippy::unwrap_used)]

use jobniac_cart::{Criterion, SplitMix64};

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-14, "{a} != {b}");
}

#[test]
fn gini_of_known_distributions_is_exact() {
    // 1 - sum p^2
    assert_eq!(Criterion::Gini.impurity(&[1.0, 1.0]), 0.5);
    assert_eq!(Criterion::Gini.impurity(&[1.0, 1.0, 1.0, 1.0]), 0.75);
    assert_eq!(Criterion::Gini.impurity(&[3.0, 1.0]), 0.375); // 1 - (9/16 + 1/16)
    assert_eq!(Criterion::Gini.impurity(&[1.0, 2.0, 1.0]), 0.625); // 1 - (1/16 + 4/16 + 1/16)
    assert_eq!(Criterion::Gini.impurity(&[5.0, 0.0]), 0.0);
    assert_eq!(Criterion::Gini.impurity(&[0.0, 0.0]), 0.0);
    assert_eq!(Criterion::Gini.impurity(&[]), 0.0);
    // Scale invariance: weights 2.5 each behave like 1 each.
    assert_eq!(Criterion::Gini.impurity(&[2.5, 2.5]), 0.5);
}

#[test]
fn entropy_of_known_distributions() {
    // Powers of two are exact: -(1/2 * -1 + 1/2 * -1) = 1, four classes give 2 bits.
    assert_eq!(Criterion::Entropy.impurity(&[1.0, 1.0]), 1.0);
    assert_eq!(Criterion::Entropy.impurity(&[1.0, 1.0, 1.0, 1.0]), 2.0);
    // -(1/4 * -2 + 1/2 * -1 + 1/4 * -2) = 1.5
    assert_eq!(Criterion::Entropy.impurity(&[1.0, 2.0, 1.0]), 1.5);
    assert_eq!(Criterion::Entropy.impurity(&[7.0, 0.0]), 0.0);
    assert_eq!(Criterion::Entropy.impurity(&[]), 0.0);
    // -(3/4 log2 3/4 + 1/4 log2 1/4) = 2 - 3/4 log2 3
    close(
        Criterion::Entropy.impurity(&[3.0, 1.0]),
        0.811_278_124_459_132_9,
    );
    // -(1/3 log2 1/3 + 2/3 log2 2/3) = log2 3 - 2/3
    close(
        Criterion::Entropy.impurity(&[1.0, 2.0]),
        0.918_295_834_054_489_6,
    );
    // Uniform over three classes is log2 3.
    close(
        Criterion::Entropy.impurity(&[2.0, 2.0, 2.0]),
        1.584_962_500_721_156_2,
    );
}

#[test]
fn criterion_names_round_trip() {
    for c in [Criterion::Gini, Criterion::Entropy] {
        assert_eq!(Criterion::parse(c.as_str()), Some(c));
    }
    assert_eq!(Criterion::parse("GINI"), None);
}

#[test]
fn splitmix64_matches_the_reference_sequence() {
    // Reference outputs for seed 0 (Vigna's splitmix64.c).
    let mut rng = SplitMix64::new(0);
    assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
    assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    assert_eq!(rng.next_u64(), 0x06c4_5d18_8009_454f);
}

#[test]
fn splitmix64_helpers_stay_in_range() {
    let mut rng = SplitMix64::new(42);
    for bound in [1usize, 2, 3, 10, 1000] {
        for _ in 0..200 {
            assert!(rng.next_below(bound) < bound);
        }
    }
    assert_eq!(rng.next_below(0), 0);
    for _ in 0..200 {
        let f = rng.next_f64();
        assert!((0.0..1.0).contains(&f));
    }
    let mut items: Vec<u32> = (0..50).collect();
    rng.shuffle(&mut items);
    let mut sorted = items.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..50).collect::<Vec<u32>>());
    assert_ne!(items, sorted);
}
