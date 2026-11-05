//! Confusion matrix and derived metrics, hand-computed.
#![allow(clippy::unwrap_used)]

mod common;

use common::names;
use jobniac_cart::{CartError, ConfusionMatrix};

fn matrix() -> ConfusionMatrix {
    // truth -> prediction:  a: a a b | b: b b b c | c: c a
    let classes: Vec<String> = (0..3).map(|i| format!("k{i}")).collect();
    let pairs = [
        (0, 0),
        (0, 0),
        (0, 1),
        (1, 1),
        (1, 1),
        (1, 1),
        (1, 2),
        (2, 2),
        (2, 0),
    ];
    ConfusionMatrix::from_pairs(&classes, pairs).unwrap()
}

#[test]
fn counts_and_scores_match_the_hand_computation() {
    // Rows are truth, columns prediction:
    //        k0 k1 k2
    //   k0    2  1  0     support 3
    //   k1    0  3  1     support 4
    //   k2    1  0  1     support 2
    // predicted totals 3, 4, 2; correct 6 of 9.
    let m = matrix();
    assert_eq!(m.total(), 9);
    assert_eq!((m.count(0, 1), m.count(1, 2), m.count(2, 0)), (1, 1, 1));
    assert_eq!(m.count(5, 0), 0);
    assert_eq!((m.support(0), m.support(1), m.support(2)), (3, 4, 2));
    assert_eq!(m.accuracy(), 6.0 / 9.0);
    // precision = tp / predicted: 2/3, 3/4, 1/2; recall = tp / support: 2/3, 3/4, 1/2.
    assert_eq!(
        (m.precision(0), m.precision(1), m.precision(2)),
        (2.0 / 3.0, 0.75, 0.5)
    );
    assert_eq!(
        (m.recall(0), m.recall(1), m.recall(2)),
        (2.0 / 3.0, 0.75, 0.5)
    );
    // f1 = 2 tp / (predicted + support): 4/6, 6/8, 2/4.
    assert_eq!((m.f1(0), m.f1(1), m.f1(2)), (4.0 / 6.0, 0.75, 0.5));
    assert!((m.macro_f1() - 23.0 / 36.0).abs() < 1e-15);
}

#[test]
fn render_is_a_plain_text_table() {
    let expected = "\
truth\\pred k0 k1 k2
k0          2  1  0
k1          0  3  1
k2          1  0  1

class      precision    recall        f1   support
k0            0.6667    0.6667    0.6667         3
k1            0.7500    0.7500    0.7500         4
k2            0.5000    0.5000    0.5000         2

accuracy 0.6667
macro-f1 0.6389
";
    assert_eq!(matrix().render(), expected);
}

#[test]
fn degenerate_cases_do_not_divide_by_zero() {
    let m = ConfusionMatrix::from_pairs(&names("c", 3), [(0, 0)]).unwrap();
    assert_eq!((m.precision(1), m.recall(1), m.f1(1)), (0.0, 0.0, 0.0));
    assert_eq!(m.accuracy(), 1.0);
    assert!((m.macro_f1() - 1.0 / 3.0).abs() < 1e-15);
    let empty = ConfusionMatrix::from_pairs(&names("c", 2), std::iter::empty()).unwrap();
    assert_eq!((empty.accuracy(), empty.macro_f1()), (0.0, 0.0));
    assert!(empty.render().contains("accuracy 0.0000"));
}

#[test]
fn out_of_range_labels_are_rejected() {
    let r = ConfusionMatrix::from_pairs(&names("c", 2), [(0, 2)]);
    assert!(matches!(
        r,
        Err(CartError::LabelOutOfRange {
            label: 2,
            class_count: 2
        })
    ));
    let r = ConfusionMatrix::from_pairs(&names("c", 2), [(7, 0)]);
    assert!(matches!(
        r,
        Err(CartError::LabelOutOfRange { label: 7, .. })
    ));
}
