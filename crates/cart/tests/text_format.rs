//! The model text format: exact round trips and rejection of hostile input.
#![allow(clippy::unwrap_used)]

mod common;

use common::{names, one_feature, synthetic};
use jobniac_cart::{
    CartError, Criterion, Dataset, HEADER, MAX_MODEL_BYTES, SplitMix64, TrainConfig, Tree, prune,
    train,
};

fn model() -> Tree {
    let cfg = TrainConfig {
        criterion: Criterion::Entropy,
        max_depth: Some(6),
        min_samples_leaf: 2,
        min_impurity_decrease: 0.0005,
        class_weights: Some(vec![1.0, 2.5, 0.1]),
        ..TrainConfig::default()
    };
    train(&synthetic(8, 150), &cfg).unwrap()
}

fn reject(text: &str) -> CartError {
    Tree::from_text(text).expect_err("text should be rejected")
}

/// Replaces one 0-based line.
fn with_line(text: &str, index: usize, replacement: &str) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines[index] = replacement;
    lines.join("\n")
}

fn line_of(text: &str, index: usize) -> String {
    text.split('\n').nth(index).unwrap().to_owned()
}

// Layout: 0 header, 1 features, 2 classes, 3 criterion, 4 max_depth, 5 min_samples_split,
// 6 min_samples_leaf, 7 min_impurity_decrease, 8 class_weights, 9 nodes, 10.. node lines, `end`.
const FIRST_NODE: usize = 10;

#[test]
fn round_trip_is_exact() {
    let tree = model();
    let text = tree.to_text();
    assert!(text.starts_with(
        "jobniac-cart 1\nfeatures 5 f0 f1 f2 f3 f4\nclasses 3 class0 class1 class2\n"
    ));
    assert!(text.ends_with("end\n"));
    let back = Tree::from_text(&text).unwrap();
    assert_eq!(back, tree);
    assert_eq!(back.to_text(), text);
    let data = synthetic(9, 100);
    for i in 0..data.len() {
        assert_eq!(
            back.predict(data.row(i)).unwrap(),
            tree.predict(data.row(i)).unwrap()
        );
        assert_eq!(
            back.predict_proba(data.row(i)).unwrap(),
            tree.predict_proba(data.row(i)).unwrap()
        );
    }
    // Pruned trees and single leaves round-trip too.
    let pruned = prune(&tree, 0.02).unwrap();
    assert_eq!(Tree::from_text(&pruned.to_text()).unwrap(), pruned);
    let leaf = train(&one_feature(&[(1.0, 0)], 1), &TrainConfig::default()).unwrap();
    assert_eq!(Tree::from_text(&leaf.to_text()).unwrap(), leaf);
}

#[test]
fn floats_are_written_as_their_bit_patterns() {
    let data = one_feature(&[(0.1, 0), (0.7, 1)], 2);
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let t = tree.nodes()[0].split.as_ref().unwrap().threshold;
    let text = tree.to_text();
    assert!(text.contains(&format!("{:016x}", t.to_bits())), "{text}");
    // Documented layout for a stump on two points.
    let expected = format!(
        "jobniac-cart 1\nfeatures 1 x0\nclasses 2 c0 c1\ncriterion gini\nmax_depth none\n\
         min_samples_split 2\nmin_samples_leaf 1\nmin_impurity_decrease 0000000000000000\n\
         class_weights none\nnodes 3\n\
         S 2 0 {} 1 2 {} {}\nL 1 {} {}\nL 1 {} {}\nend\n",
        format_args!("{:016x}", t.to_bits()),
        format_args!("{:016x}", 1.0_f64.to_bits()),
        format_args!("{:016x}", 1.0_f64.to_bits()),
        format_args!("{:016x}", 1.0_f64.to_bits()),
        format_args!("{:016x}", 0.0_f64.to_bits()),
        format_args!("{:016x}", 0.0_f64.to_bits()),
        format_args!("{:016x}", 1.0_f64.to_bits()),
    );
    assert_eq!(text, expected);
}

#[test]
fn rejects_wrong_header_and_version() {
    let text = model().to_text();
    assert!(matches!(
        reject(""),
        CartError::InvalidModel { line: 1, .. }
    ));
    assert!(matches!(
        reject("\n"),
        CartError::InvalidModel { line: 1, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 0, "jobniac-cart 2")),
        CartError::InvalidModel { line: 1, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 0, "jobniac-cart 0")),
        CartError::InvalidModel { line: 1, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 0, "jobniac-cart 1 ")),
        CartError::InvalidModel { line: 1, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 0, "jobniac-tree 1")),
        CartError::InvalidModel { line: 1, .. }
    ));
    assert!(matches!(
        reject(&text.replacen("jobniac-cart 1", "Jobniac-Cart 1", 1)),
        CartError::InvalidModel { .. }
    ));
    assert!(HEADER == "jobniac-cart 1");
}

#[test]
fn rejects_every_truncation() {
    let text = model().to_text();
    for cut in 0..text.len() {
        if text.is_char_boundary(cut) {
            assert!(
                Tree::from_text(&text[..cut]).is_err(),
                "prefix of {cut} bytes was accepted"
            );
        }
    }
    assert!(Tree::from_text(&text).is_ok());
}

#[test]
fn rejects_counts_that_do_not_match() {
    let text = model().to_text();
    let features = line_of(&text, 1);
    assert!(matches!(
        reject(&with_line(
            &text,
            1,
            &features.replacen("features 5", "features 4", 1)
        )),
        CartError::InvalidModel { line: 2, .. }
    ));
    assert!(matches!(
        reject(&with_line(
            &text,
            1,
            &features.replacen("features 5", "features 6", 1)
        )),
        CartError::InvalidModel { line: 2, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 1, "features 0")),
        CartError::InvalidModel { line: 2, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 2, "classes 3 a b")),
        CartError::InvalidModel { line: 3, .. }
    ));
    // Class weights must match the class count.
    assert!(matches!(
        reject(&with_line(
            &text,
            8,
            "class_weights 2 3ff0000000000000 3ff0000000000000"
        )),
        CartError::InvalidModel { line: 9, .. }
    ));
    // Too few and too many nodes.
    let nodes = text
        .split('\n')
        .nth(9)
        .unwrap()
        .strip_prefix("nodes ")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert!(matches!(
        reject(&with_line(&text, 9, &format!("nodes {}", nodes + 1))),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 9, &format!("nodes {}", nodes - 1))),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 9, "nodes 0")),
        CartError::InvalidModel { .. }
    ));
    // A node line with a missing or extra count.
    let node = line_of(&text, FIRST_NODE);
    assert!(matches!(
        reject(&with_line(
            &text,
            FIRST_NODE,
            node.rsplit_once(' ').unwrap().0
        )),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(
            &text,
            FIRST_NODE,
            &format!("{node} 0000000000000000")
        )),
        CartError::InvalidModel { .. }
    ));
}

#[test]
fn rejects_bad_child_indices() {
    let text = model().to_text();
    let root = line_of(&text, FIRST_NODE);
    let fields: Vec<&str> = root.split(' ').collect();
    assert_eq!(fields[0], "S");
    let rebuild = |left: &str, right: &str| {
        let mut f = fields.clone();
        f[4] = left;
        f[5] = right;
        with_line(&text, FIRST_NODE, &f.join(" "))
    };
    let n = text
        .lines()
        .filter(|l| l.starts_with("S ") || l.starts_with("L "))
        .count();
    assert!(matches!(
        reject(&rebuild("0", fields[5])),
        CartError::InvalidModel { line: 11, .. }
    )); // points back at the root
    assert!(matches!(
        reject(&rebuild(fields[4], "0")),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&rebuild(&n.to_string(), fields[5])),
        CartError::InvalidModel { .. }
    )); // out of range
    assert!(matches!(
        reject(&rebuild("99999999999999999999999", fields[5])),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&rebuild(fields[4], fields[4])),
        CartError::InvalidModel { .. }
    )); // same child twice
    assert!(matches!(
        reject(&rebuild("-1", fields[5])),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&rebuild("+1", fields[5])),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&rebuild("01", fields[5])),
        CartError::InvalidModel { .. }
    ));
    // Swapping to 1 and 3 leaves node 2 without a parent.
    let mut leaf_pair = fields.clone();
    leaf_pair[4] = "1";
    leaf_pair[5] = "3";
    assert!(matches!(
        reject(&with_line(&text, FIRST_NODE, &leaf_pair.join(" "))),
        CartError::InvalidModel { .. }
    ));
    // A node that is the child of two parents: point the first inner split's right child at its left.
    let second = text
        .split('\n')
        .enumerate()
        .skip(FIRST_NODE + 1)
        .find(|(_, l)| l.starts_with("S "))
        .map(|(i, l)| (i, l.to_owned()))
        .unwrap();
    // Non-increasing: a child pointing at its own index.
    let mut own: Vec<String> = second.1.split(' ').map(str::to_owned).collect();
    own[4] = (second.0 - FIRST_NODE).to_string();
    assert!(matches!(
        reject(&with_line(&text, second.0, &own.join(" "))),
        CartError::InvalidModel { .. }
    ));
}

#[test]
fn rejects_non_finite_and_malformed_numbers() {
    let text = model().to_text();
    let root = line_of(&text, FIRST_NODE);
    let fields: Vec<String> = root.split(' ').map(str::to_owned).collect();
    let mutate = |index: usize, value: &str| {
        let mut f = fields.clone();
        f[index] = value.to_owned();
        with_line(&text, FIRST_NODE, &f.join(" "))
    };
    for bad in [
        "7ff8000000000000",  // NaN
        "fff8000000000000",  // NaN, sign set
        "7ff0000000000000",  // +inf
        "fff0000000000000",  // -inf
        "3FF0000000000000",  // upper case
        "3ff000000000000",   // 15 digits
        "3ff00000000000000", // 17 digits
        "3ff000000000000g",
        "0x3ff0000000000000",
        "1.0",
        "",
    ] {
        assert!(
            matches!(reject(&mutate(3, bad)), CartError::InvalidModel { .. }),
            "threshold {bad:?}"
        );
        assert!(
            matches!(reject(&mutate(6, bad)), CartError::InvalidModel { .. }),
            "count {bad:?}"
        );
    }
    // Negative counts are not class counts.
    assert!(matches!(
        reject(&mutate(6, "bff0000000000000")),
        CartError::InvalidModel { .. }
    ));
    // Config floats and the weights line.
    assert!(matches!(
        reject(&with_line(
            &text,
            7,
            "min_impurity_decrease 7ff8000000000000"
        )),
        CartError::InvalidModel { line: 8, .. }
    ));
    assert!(matches!(
        reject(&with_line(
            &text,
            7,
            "min_impurity_decrease bff0000000000000"
        )),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(
            &text,
            8,
            "class_weights 3 3ff0000000000000 7ff0000000000000 3ff0000000000000"
        )),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(
            &text,
            8,
            "class_weights 3 3ff0000000000000 0000000000000000 3ff0000000000000"
        )),
        CartError::InvalidModel { .. }
    ));
    // Integers.
    assert!(matches!(
        reject(&mutate(1, "0")),
        CartError::InvalidModel { .. }
    )); // zero samples
    assert!(matches!(
        reject(&mutate(2, "99")),
        CartError::InvalidModel { .. }
    )); // feature out of range
    assert!(matches!(
        reject(&mutate(1, "")),
        CartError::InvalidModel { .. }
    ));
}

#[test]
fn rejects_inconsistent_samples_and_config() {
    let text = model().to_text();
    let root = line_of(&text, FIRST_NODE);
    let mut f: Vec<String> = root.split(' ').map(str::to_owned).collect();
    let samples: usize = f[1].parse().unwrap();
    f[1] = (samples + 1).to_string();
    assert!(matches!(
        reject(&with_line(&text, FIRST_NODE, &f.join(" "))),
        CartError::InvalidModel { .. }
    ));
    for bad in [
        (3, "criterion variance"),
        (3, "criterion"),
        (4, "max_depth -1"),
        (4, "max_depth"),
        (5, "min_samples_split 1"),
        (6, "min_samples_leaf 0"),
        (5, "min_samples_leaf 2"), // wrong keyword for the line
    ] {
        assert!(
            matches!(
                reject(&with_line(&text, bad.0, bad.1)),
                CartError::InvalidModel { .. }
            ),
            "{bad:?}"
        );
    }
    // Duplicate and invalid names.
    assert!(matches!(
        reject(&with_line(&text, 1, "features 2 a a")),
        CartError::InvalidModel { line: 2, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 1, "features 2 a b/c")),
        CartError::InvalidModel { line: 2, .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 1, "features 2 a  b")),
        CartError::InvalidModel { .. }
    )); // double space
}

#[test]
fn rejects_trailing_data_and_line_ending_changes() {
    let text = model().to_text();
    for suffix in ["x", "\n", "end\n", "\n\n", " ", "jobniac-cart 1\n"] {
        assert!(
            matches!(
                reject(&format!("{text}{suffix}")),
                CartError::InvalidModel { .. }
            ),
            "{suffix:?}"
        );
    }
    assert!(Tree::from_text(&text.replace('\n', "\r\n")).is_err());
    assert!(Tree::from_text(text.trim_end_matches('\n')).is_err());
    assert!(Tree::from_text(&text.replacen("end\n", "End\n", 1)).is_err());
}

#[test]
fn rejects_oversized_inputs() {
    let huge = "a".repeat(MAX_MODEL_BYTES + 1);
    assert!(matches!(
        reject(&huge),
        CartError::InvalidModel { line: 0, .. }
    ));
    let text = model().to_text();
    assert!(matches!(
        reject(&with_line(&text, 9, "nodes 1000001")),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 9, "nodes 99999999999999999999999999")),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 1, "features 4097")),
        CartError::InvalidModel { .. }
    ));
    assert!(matches!(
        reject(&with_line(&text, 2, "classes 1025")),
        CartError::InvalidModel { .. }
    ));
    // A declared node count far beyond the data fails on the missing lines without allocating them.
    assert!(matches!(
        reject(&with_line(&text, 9, "nodes 1000000")),
        CartError::InvalidModel { .. }
    ));
}

#[test]
fn mutated_models_never_panic() {
    let text = model().to_text();
    let bytes = text.as_bytes();
    let alphabet = b" \n0123456789abcdefSLx-+.\t";
    let mut rng = SplitMix64::new(1234);
    for _ in 0..4000 {
        let mut copy = bytes.to_vec();
        for _ in 0..1 + rng.next_below(3) {
            let at = rng.next_below(copy.len());
            copy[at] = alphabet[rng.next_below(alphabet.len())];
        }
        let mutated = String::from_utf8(copy).unwrap();
        if let Ok(tree) = Tree::from_text(&mutated) {
            // Whatever parses must be safe to use and to write again.
            let _ = tree.to_text();
            let _ = tree.depth();
            let _ = tree.feature_importances();
            let _ = tree.predict(&[0.0; 5]);
            let _ = prune(&tree, 0.1);
        }
    }
    // Random garbage lines.
    for _ in 0..500 {
        let len = rng.next_below(300);
        let junk: String = (0..len)
            .map(|_| char::from(alphabet[rng.next_below(alphabet.len())]))
            .collect();
        let _ = Tree::from_text(&junk);
    }
}

#[test]
fn dataset_names_survive_in_the_model() {
    let mut data = Dataset::new(
        vec!["a.b-c_1".to_owned()],
        vec!["x-1".to_owned(), "y.2".to_owned()],
    )
    .unwrap();
    data.push(&[0.0], 0).unwrap();
    data.push(&[1.0], 1).unwrap();
    let tree = train(&data, &TrainConfig::default()).unwrap();
    let back = Tree::from_text(&tree.to_text()).unwrap();
    assert_eq!(back.feature_names(), &["a.b-c_1".to_owned()]);
    assert_eq!(back.class_names(), &["x-1".to_owned(), "y.2".to_owned()]);
    let _ = names("z", 1);
}
