//! Performance sanity check, ignored by default. Run it in release mode:
//!
//! ```text
//! cargo test -p jobniac-cart --release --test perf -- --ignored --nocapture
//! ```
//!
//! Training costs about `O(depth * features * rows * log rows)`; 20,000 rows by 30 features is a
//! fraction of a second to a few seconds in release, depending on how noisy the labels are.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::names;
use jobniac_cart::{Dataset, SplitMix64, TrainConfig, train};

#[test]
#[ignore = "timing test; run with --release -- --ignored"]
fn trains_20000_rows_by_30_features_quickly() {
    let mut rng = SplitMix64::new(77);
    let mut data = Dataset::new(names("f", 30), names("c", 4)).unwrap();
    for _ in 0..20_000 {
        let mut row = [0.0; 30];
        for v in &mut row {
            *v = rng.next_f64();
        }
        // The label depends on three features, plus 10% label noise.
        let mut label = usize::from(row[0] > 0.5) * 2 + usize::from(row[1] + row[2] > 1.0);
        if rng.next_below(10) == 0 {
            label = rng.next_below(4);
        }
        data.push(&row, label).unwrap();
    }
    let started = Instant::now();
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let elapsed = started.elapsed();
    println!(
        "trained {} nodes, depth {}, in {elapsed:?}",
        tree.node_count(),
        tree.depth()
    );
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

#[test]
#[ignore = "timing test; run with --release -- --ignored"]
fn selects_alpha_on_20000_noisy_rows() {
    let mut rng = SplitMix64::new(78);
    let mut data = Dataset::new(names("f", 30), names("c", 4)).unwrap();
    for _ in 0..20_000 {
        let mut row = [0.0; 30];
        for v in &mut row {
            *v = rng.next_f64();
        }
        let mut label = usize::from(row[0] > 0.5) * 2 + usize::from(row[1] + row[2] > 1.0);
        if rng.next_below(5) == 0 {
            label = rng.next_below(4);
        }
        data.push(&row, label).unwrap();
    }
    let started = Instant::now();
    let sel = jobniac_cart::select_alpha(&data, &TrainConfig::default(), 5, 1).unwrap();
    let elapsed = started.elapsed();
    println!("{} candidates in {elapsed:?}", sel.scores.len());
    assert!(elapsed < Duration::from_secs(120), "took {elapsed:?}");
}
