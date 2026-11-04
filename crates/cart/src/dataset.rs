//! Labelled, weighted training data and deterministic stratified splitting.

use crate::error::CartError;
use crate::rng::SplitMix64;

/// Maximum number of features in a dataset or model.
pub const MAX_FEATURES: usize = 4096;
/// Maximum number of classes in a dataset or model.
pub const MAX_CLASSES: usize = 1024;
/// Maximum length in bytes of a feature or class name.
pub const MAX_NAME_LEN: usize = 128;

/// Checks that `name` is non-empty, at most [`MAX_NAME_LEN`] bytes and uses only `[A-Za-z0-9_.-]`.
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Validates a list of names: each valid, all unique, count within `1..=max`.
pub(crate) fn validate_names(
    kind: &'static str,
    names: &[String],
    max: usize,
    empty: CartError,
) -> Result<(), CartError> {
    if names.is_empty() {
        return Err(empty);
    }
    if names.len() > max {
        return Err(CartError::LimitExceeded {
            what: if kind == "feature" {
                "features"
            } else {
                "classes"
            },
            max,
        });
    }
    for (i, name) in names.iter().enumerate() {
        if !is_valid_name(name) {
            return Err(CartError::InvalidName {
                kind,
                name: name.clone(),
            });
        }
        if names[..i].contains(name) {
            return Err(CartError::DuplicateName {
                kind,
                name: name.clone(),
            });
        }
    }
    Ok(())
}

/// A table of numeric feature rows with a class label and a weight per row.
///
/// Rows are stored in insertion order and all splitting and training operates in that order, so a
/// dataset built the same way always trains the same tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    feature_names: Vec<String>,
    class_names: Vec<String>,
    values: Vec<f64>,
    labels: Vec<usize>,
    weights: Vec<f64>,
}

impl Dataset {
    /// Creates an empty dataset.
    ///
    /// Names must match `[A-Za-z0-9_.-]{1,128}` and be unique within their list; there must be
    /// between one and [`MAX_FEATURES`] features and between one and [`MAX_CLASSES`] classes.
    ///
    /// # Errors
    /// Returns [`CartError`] when a name or a count is invalid.
    pub fn new(feature_names: Vec<String>, class_names: Vec<String>) -> Result<Self, CartError> {
        validate_names(
            "feature",
            &feature_names,
            MAX_FEATURES,
            CartError::NoFeatures,
        )?;
        validate_names("class", &class_names, MAX_CLASSES, CartError::NoClasses)?;
        Ok(Self {
            feature_names,
            class_names,
            values: Vec::new(),
            labels: Vec::new(),
            weights: Vec::new(),
        })
    }

    /// Appends a row with weight 1.0.
    ///
    /// # Errors
    /// See [`Dataset::push_weighted`].
    pub fn push(&mut self, features: &[f64], label: usize) -> Result<(), CartError> {
        self.push_weighted(features, label, 1.0)
    }

    /// Appends a row. Nothing is stored when an error is returned.
    ///
    /// # Errors
    /// Rejects a wrong feature count, a NaN or infinite feature, a label not below the class count,
    /// and a weight that is not positive and finite.
    pub fn push_weighted(
        &mut self,
        features: &[f64],
        label: usize,
        weight: f64,
    ) -> Result<(), CartError> {
        if features.len() != self.feature_names.len() {
            return Err(CartError::FeatureCountMismatch {
                expected: self.feature_names.len(),
                actual: features.len(),
            });
        }
        if let Some(index) = features.iter().position(|v| !v.is_finite()) {
            return Err(CartError::NonFiniteFeature { index });
        }
        if label >= self.class_names.len() {
            return Err(CartError::LabelOutOfRange {
                label,
                class_count: self.class_names.len(),
            });
        }
        if !(weight.is_finite() && weight > 0.0) {
            return Err(CartError::InvalidWeight { weight });
        }
        self.values.extend_from_slice(features);
        self.labels.push(label);
        self.weights.push(weight);
        Ok(())
    }

    /// Number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    /// True when there are no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// Number of features per row.
    #[must_use]
    pub fn feature_count(&self) -> usize {
        self.feature_names.len()
    }

    /// Number of classes.
    #[must_use]
    pub fn class_count(&self) -> usize {
        self.class_names.len()
    }

    /// Feature names in column order.
    #[must_use]
    pub fn feature_names(&self) -> &[String] {
        &self.feature_names
    }

    /// Class names in label order.
    #[must_use]
    pub fn class_names(&self) -> &[String] {
        &self.class_names
    }

    /// Feature values of row `i`, or an empty slice when `i` is out of range.
    #[must_use]
    pub fn row(&self, i: usize) -> &[f64] {
        let f = self.feature_names.len();
        let range = i
            .checked_mul(f)
            .and_then(|start| start.checked_add(f).map(|end| start..end));
        range.and_then(|r| self.values.get(r)).unwrap_or(&[])
    }

    /// Value of feature `feature` in row `row` (NaN is never stored; 0.0 when out of range).
    #[must_use]
    pub(crate) fn value(&self, row: usize, feature: usize) -> f64 {
        row.checked_mul(self.feature_names.len())
            .and_then(|start| start.checked_add(feature))
            .and_then(|at| self.values.get(at))
            .copied()
            .unwrap_or(0.0)
    }

    /// Label of row `i` (0 when out of range).
    #[must_use]
    pub fn label(&self, i: usize) -> usize {
        self.labels.get(i).copied().unwrap_or(0)
    }

    /// All labels in row order.
    #[must_use]
    pub fn labels(&self) -> &[usize] {
        &self.labels
    }

    /// Weight of row `i` (1.0 when out of range).
    #[must_use]
    pub fn weight(&self, i: usize) -> f64 {
        self.weights.get(i).copied().unwrap_or(1.0)
    }

    /// A new dataset holding the given rows in the given order (repeats allowed).
    ///
    /// # Errors
    /// Returns [`CartError::RowOutOfRange`] for an index not below [`Dataset::len`].
    pub fn subset(&self, indices: &[usize]) -> Result<Dataset, CartError> {
        let f = self.feature_names.len();
        let mut out = Dataset {
            feature_names: self.feature_names.clone(),
            class_names: self.class_names.clone(),
            values: Vec::with_capacity(indices.len() * f),
            labels: Vec::with_capacity(indices.len()),
            weights: Vec::with_capacity(indices.len()),
        };
        for &i in indices {
            if i >= self.len() {
                return Err(CartError::RowOutOfRange {
                    row: i,
                    len: self.len(),
                });
            }
            out.values.extend_from_slice(self.row(i));
            out.labels.push(self.label(i));
            out.weights.push(self.weight(i));
        }
        Ok(out)
    }

    /// Row indices of each class, ascending, indexed by label.
    fn rows_by_class(&self) -> Vec<Vec<usize>> {
        let mut by_class = vec![Vec::new(); self.class_names.len()];
        for (i, &label) in self.labels.iter().enumerate() {
            if let Some(rows) = by_class.get_mut(label) {
                rows.push(i);
            }
        }
        by_class
    }

    /// Splits into `(train, test)` keeping class proportions.
    ///
    /// Each class's rows are shuffled with a Fisher-Yates shuffle driven by [`SplitMix64`] seeded
    /// with `seed` (classes visited in label order). A class of `c` rows sends
    /// `round(c * test_fraction)` rows to the test set, clamped to `1..=c-1` when `c >= 2`; a class
    /// with one row goes to the training set.
    ///
    /// The training set is never empty. If no class has two rows the test set would be empty and
    /// an error is returned instead.
    ///
    /// # Errors
    /// `test_fraction` must be finite and strictly between 0 and 1, and the dataset needs at least
    /// two rows.
    pub fn stratified_split(
        &self,
        seed: u64,
        test_fraction: f64,
    ) -> Result<(Dataset, Dataset), CartError> {
        if !(test_fraction.is_finite() && test_fraction > 0.0 && test_fraction < 1.0) {
            return Err(CartError::InvalidArgument {
                reason: format!("test fraction {test_fraction} is not strictly between 0 and 1"),
            });
        }
        if self.len() < 2 {
            return Err(CartError::InvalidArgument {
                reason: "a split needs at least two rows".to_owned(),
            });
        }
        let mut rng = SplitMix64::new(seed);
        let mut is_test = vec![false; self.len()];
        for mut rows in self.rows_by_class() {
            let count = rows.len();
            rng.shuffle(&mut rows);
            if count < 2 {
                continue;
            }
            let wanted = (count as f64 * test_fraction).round() as usize;
            let n_test = wanted.clamp(1, count - 1);
            for &row in rows.iter().take(n_test) {
                if let Some(flag) = is_test.get_mut(row) {
                    *flag = true;
                }
            }
        }
        if !is_test.contains(&true) {
            return Err(CartError::InvalidArgument {
                reason: "every class has a single row, so the test set would be empty".to_owned(),
            });
        }
        let train: Vec<usize> = (0..self.len()).filter(|&i| !is_test[i]).collect();
        let test: Vec<usize> = (0..self.len()).filter(|&i| is_test[i]).collect();
        Ok((self.subset(&train)?, self.subset(&test)?))
    }

    /// Partitions the rows into `k` stratified folds of row indices (each ascending).
    ///
    /// Each class's rows are shuffled as in [`Dataset::stratified_split`], then dealt round-robin
    /// over the folds with one deal counter shared by all classes, so fold sizes differ by at most
    /// one and every class is spread as evenly as possible.
    ///
    /// # Errors
    /// `k` must be at least 2 and at most the number of rows.
    pub fn stratified_folds(&self, k: usize, seed: u64) -> Result<Vec<Vec<usize>>, CartError> {
        if k < 2 || k > self.len() {
            return Err(CartError::InvalidArgument {
                reason: format!(
                    "fold count {k} must be between 2 and the row count {}",
                    self.len()
                ),
            });
        }
        let mut rng = SplitMix64::new(seed);
        let mut folds = vec![Vec::new(); k];
        let mut deal = 0usize;
        for mut rows in self.rows_by_class() {
            rng.shuffle(&mut rows);
            for row in rows {
                folds[deal % k].push(row);
                deal += 1;
            }
        }
        for fold in &mut folds {
            fold.sort_unstable();
        }
        Ok(folds)
    }
}
