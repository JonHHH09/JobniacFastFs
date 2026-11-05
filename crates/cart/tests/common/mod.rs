//! Helpers shared by the integration tests.
#![allow(dead_code, clippy::unwrap_used)]

use jobniac_cart::{Dataset, SplitMix64};

/// Names `prefix0`, `prefix1`, ...
pub fn names(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i}")).collect()
}

/// A dataset with one feature `x` and the given `(x, label)` rows, `classes` classes.
pub fn one_feature(rows: &[(f64, usize)], classes: usize) -> Dataset {
    let mut data = Dataset::new(names("x", 1), names("c", classes)).unwrap();
    for &(x, y) in rows {
        data.push(&[x], y).unwrap();
    }
    data
}

/// The fixed synthetic dataset behind the golden file: 300 rows, 5 features, 3 classes.
///
/// Class `c` is drawn uniformly; feature `j` is `centre[c][j] + noise` with noise in `[-1.5, 1.5)`
/// and is rounded to two decimals (so values repeat and ties between rows occur). Feature 4 is pure
/// noise. Only `+ - * /`, `round` and SplitMix64 are used, so the data is identical everywhere.
pub fn synthetic(seed: u64, rows: usize) -> Dataset {
    const CENTRES: [[f64; 4]; 3] = [
        [0.0, 2.0, -1.0, 1.0],
        [1.5, 0.0, 1.0, 1.0],
        [3.0, 1.0, 0.0, -1.0],
    ];
    let mut rng = SplitMix64::new(seed);
    let mut data = Dataset::new(names("f", 5), names("class", 3)).unwrap();
    for _ in 0..rows {
        let class = rng.next_below(3);
        let mut row = [0.0; 5];
        for (j, slot) in row.iter_mut().enumerate() {
            let centre = CENTRES[class].get(j).copied().unwrap_or(0.0);
            let value = centre + (rng.next_f64() - 0.5) * 3.0;
            *slot = (value * 100.0).round() / 100.0;
        }
        data.push(&row, class).unwrap();
    }
    data
}
