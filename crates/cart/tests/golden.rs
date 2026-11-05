//! Byte-for-byte golden model: a tree trained on a fixed synthetic dataset must serialise to the
//! committed file `golden/synthetic_tree.txt`.
//!
//! The dataset comes from SplitMix64 (see `common::synthetic`), so nothing but the training code
//! can change the output. To regenerate the file after an intentional change of the algorithm or
//! the format, run
//!
//! ```text
//! JOBNIAC_CART_REGEN=1 cargo test -p jobniac-cart --test golden
//! ```
//!
//! and review the diff of `crates/cart/tests/golden/` before committing it. The run writes the
//! files and then fails on purpose; rerun without the variable. It refuses to run when `CI` is set.
#![allow(clippy::unwrap_used)]

mod common;

use jobniac_cart::{Criterion, TrainConfig, Tree, train};

const GOLDEN_GINI: &str = include_str!("golden/synthetic_tree.txt");
const GOLDEN_ENTROPY: &str = include_str!("golden/synthetic_tree_entropy.txt");

fn check(file: &str, expected: &str, actual: &str) {
    if std::env::var_os("JOBNIAC_CART_REGEN").is_some_and(|v| v == "1") {
        assert!(
            std::env::var_os("CI").is_none(),
            "refusing to regenerate golden files when CI is set"
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(file);
        std::fs::write(path, actual).unwrap();
        panic!("golden files regenerated; rerun without JOBNIAC_CART_REGEN");
    }
    assert_eq!(
        actual, expected,
        "{file} differs; if the change is intended, regenerate it (see the module docs)"
    );
}

fn config(criterion: Criterion) -> TrainConfig {
    TrainConfig {
        criterion,
        min_samples_leaf: 3,
        min_impurity_decrease: 0.001,
        ..TrainConfig::default()
    }
}

#[test]
fn gini_tree_matches_the_golden_file() {
    let data = common::synthetic(2026, 300);
    let tree = train(&data, &config(Criterion::Gini)).unwrap();
    check("synthetic_tree.txt", GOLDEN_GINI, &tree.to_text());
}

#[test]
fn entropy_tree_matches_the_golden_file() {
    let data = common::synthetic(2026, 300);
    let tree = train(&data, &config(Criterion::Entropy)).unwrap();
    check(
        "synthetic_tree_entropy.txt",
        GOLDEN_ENTROPY,
        &tree.to_text(),
    );
}

#[test]
fn golden_files_parse_and_predict_like_a_fresh_tree() {
    let data = common::synthetic(2026, 300);
    for (criterion, text) in [
        (Criterion::Gini, GOLDEN_GINI),
        (Criterion::Entropy, GOLDEN_ENTROPY),
    ] {
        let loaded = Tree::from_text(text).unwrap();
        let fresh = train(&data, &config(criterion)).unwrap();
        assert_eq!(loaded, fresh);
        let mut correct = 0;
        for i in 0..data.len() {
            let p = loaded.predict(data.row(i)).unwrap();
            assert_eq!(p, fresh.predict(data.row(i)).unwrap());
            correct += usize::from(p == data.label(i));
        }
        // The classes overlap, so the model is good but not perfect.
        assert!(
            correct * 100 >= data.len() * 80,
            "training accuracy {correct}/{}",
            data.len()
        );
    }
}
