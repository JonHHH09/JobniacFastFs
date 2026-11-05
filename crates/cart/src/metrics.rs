//! Classification metrics.

use std::fmt::Write as _;

use crate::error::CartError;

/// Counts of (truth, prediction) pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfusionMatrix {
    class_names: Vec<String>,
    /// Row-major `class_count x class_count`, indexed `[truth][prediction]`.
    cells: Vec<u64>,
}

impl ConfusionMatrix {
    /// Builds the matrix from `(truth, prediction)` pairs.
    ///
    /// # Errors
    /// [`CartError::LabelOutOfRange`] when a label is not below the number of classes.
    pub fn from_pairs<I>(class_names: &[String], pairs: I) -> Result<Self, CartError>
    where
        I: IntoIterator<Item = (usize, usize)>,
    {
        let k = class_names.len();
        let mut cells = vec![0u64; k * k];
        for (truth, prediction) in pairs {
            for label in [truth, prediction] {
                if label >= k {
                    return Err(CartError::LabelOutOfRange {
                        label,
                        class_count: k,
                    });
                }
            }
            cells[truth * k + prediction] += 1;
        }
        Ok(Self {
            class_names: class_names.to_vec(),
            cells,
        })
    }

    /// Number of classes.
    #[must_use]
    pub fn class_count(&self) -> usize {
        self.class_names.len()
    }

    /// Number of pairs with the given truth and prediction (0 when out of range).
    #[must_use]
    pub fn count(&self, truth: usize, prediction: usize) -> u64 {
        let k = self.class_count();
        if truth >= k || prediction >= k {
            return 0;
        }
        self.cells[truth * k + prediction]
    }

    /// Total number of pairs.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.cells.iter().sum()
    }

    /// Number of pairs whose truth is `class` (the class's support).
    #[must_use]
    pub fn support(&self, class: usize) -> u64 {
        (0..self.class_count()).map(|p| self.count(class, p)).sum()
    }

    fn predicted(&self, class: usize) -> u64 {
        (0..self.class_count()).map(|t| self.count(t, class)).sum()
    }

    /// Fraction of pairs on the diagonal; 0 for an empty matrix.
    #[must_use]
    pub fn accuracy(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        let correct: u64 = (0..self.class_count()).map(|c| self.count(c, c)).sum();
        correct as f64 / total as f64
    }

    /// `tp / (tp + fp)`; 0 when the class is never predicted.
    #[must_use]
    pub fn precision(&self, class: usize) -> f64 {
        ratio(self.count(class, class), self.predicted(class))
    }

    /// `tp / (tp + fn)`; 0 when the class never occurs.
    #[must_use]
    pub fn recall(&self, class: usize) -> f64 {
        ratio(self.count(class, class), self.support(class))
    }

    /// `2 tp / (2 tp + fp + fn)`, the harmonic mean of precision and recall; 0 when undefined.
    #[must_use]
    pub fn f1(&self, class: usize) -> f64 {
        let tp = self.count(class, class);
        ratio(2 * tp, self.predicted(class) + self.support(class))
    }

    /// Unweighted mean of the per-class F1 over all classes (absent classes count as 0).
    #[must_use]
    pub fn macro_f1(&self) -> f64 {
        let k = self.class_count();
        if k == 0 {
            return 0.0;
        }
        let sum: f64 = (0..k).map(|c| self.f1(c)).sum();
        sum / k as f64
    }

    /// Plain-text table: the matrix (rows are truth, columns are predictions), then per-class
    /// precision, recall, F1 and support, then accuracy and macro-F1 with four decimals.
    #[must_use]
    pub fn render(&self) -> String {
        let k = self.class_count();
        let name_width = self
            .class_names
            .iter()
            .map(String::len)
            .max()
            .unwrap_or(0)
            .max("truth\\pred".len());
        let widths: Vec<usize> = (0..k)
            .map(|c| {
                let name = self.class_names[c].len();
                let cells = (0..k)
                    .map(|t| self.count(t, c).to_string().len())
                    .max()
                    .unwrap_or(1);
                name.max(cells)
            })
            .collect();
        let mut out = String::new();
        let _ = write!(out, "{:<name_width$}", "truth\\pred");
        for (c, w) in widths.iter().enumerate() {
            let _ = write!(out, " {:>w$}", self.class_names[c]);
        }
        out.push('\n');
        for t in 0..k {
            let _ = write!(out, "{:<name_width$}", self.class_names[t]);
            for (c, w) in widths.iter().enumerate() {
                let _ = write!(out, " {:>w$}", self.count(t, c));
            }
            out.push('\n');
        }
        out.push('\n');
        let _ = writeln!(
            out,
            "{:<name_width$} {:>9} {:>9} {:>9} {:>9}",
            "class", "precision", "recall", "f1", "support"
        );
        for c in 0..k {
            let _ = writeln!(
                out,
                "{:<name_width$} {:>9.4} {:>9.4} {:>9.4} {:>9}",
                self.class_names[c],
                self.precision(c),
                self.recall(c),
                self.f1(c),
                self.support(c)
            );
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "accuracy {:.4}", self.accuracy());
        let _ = writeln!(out, "macro-f1 {:.4}", self.macro_f1());
        out
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}
