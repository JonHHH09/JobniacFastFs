//! Impurity criteria and training configuration.

use crate::error::CartError;

/// Node impurity measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterion {
    /// Gini impurity `1 - sum(p_k^2)`.
    Gini,
    /// Shannon entropy in bits, `-sum(p_k * log2 p_k)`, using a portable `log2` that only needs
    /// IEEE `+ - * /` so results are identical on every platform.
    Entropy,
}

impl Criterion {
    /// Impurity of a weighted class distribution. Empty or all-zero counts give 0.
    #[must_use]
    pub fn impurity(self, counts: &[f64]) -> f64 {
        let total: f64 = counts.iter().sum();
        if total.is_nan() || total <= 0.0 || total.is_infinite() {
            return 0.0;
        }
        match self {
            Criterion::Gini => {
                let squares: f64 = counts
                    .iter()
                    .map(|&c| {
                        let p = c / total;
                        p * p
                    })
                    .sum();
                (1.0 - squares).max(0.0)
            }
            Criterion::Entropy => {
                let s: f64 = counts
                    .iter()
                    .filter(|&&c| c > 0.0)
                    .map(|&c| {
                        let p = c / total;
                        p * portable_log2(p)
                    })
                    .sum();
                (0.0 - s).max(0.0)
            }
        }
    }

    /// Lower-case name used in the model text format.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Criterion::Gini => "gini",
            Criterion::Entropy => "entropy",
        }
    }

    /// Inverse of [`Criterion::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gini" => Some(Criterion::Gini),
            "entropy" => Some(Criterion::Entropy),
            _ => None,
        }
    }
}

/// `log2(x)` for finite `x > 0`, built from `+ - * /` only (exact for powers of two).
///
/// `x = m * 2^e` with `m` in `[sqrt(1/2), sqrt(2))`; `ln m = 2 * atanh((m-1)/(m+1))` is summed as a
/// fixed 16-term series (|s| < 0.172, truncation error far below 1e-16).
pub(crate) fn portable_log2(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return f64::NAN;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let mut v = x;
    let mut exp: i64 = 0;
    if v < f64::MIN_POSITIVE {
        v *= 18_014_398_509_481_984.0; // 2^54
        exp -= 54;
    }
    let bits = v.to_bits();
    exp += ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        exp += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut term = s;
    let mut sum = 0.0;
    for k in 0..16 {
        sum += term / (2 * k + 1) as f64;
        term *= s2;
    }
    let ln_m = 2.0 * sum;
    exp as f64 + ln_m * std::f64::consts::LOG2_E
}

/// Settings for [`crate::train`].
#[derive(Debug, Clone, PartialEq)]
pub struct TrainConfig {
    /// Impurity measure used to score splits.
    pub criterion: Criterion,
    /// Maximum depth of the tree (root is depth 0); `None` means unlimited.
    pub max_depth: Option<usize>,
    /// A node with fewer rows than this is not split. At least 2.
    pub min_samples_split: usize,
    /// Every leaf must keep at least this many rows. At least 1.
    pub min_samples_leaf: usize,
    /// A split is made only when `(W_t / W_root) * (i(t) - wl/W_t * i(L) - wr/W_t * i(R))` is at
    /// least this value (Breiman / scikit-learn weighted decrease). Finite and not negative.
    pub min_impurity_decrease: f64,
    /// Optional multiplicative weight per class (length must equal the class count), each positive
    /// and finite. Multiplied into every row weight before counting.
    pub class_weights: Option<Vec<f64>>,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            criterion: Criterion::Gini,
            max_depth: None,
            min_samples_split: 2,
            min_samples_leaf: 1,
            min_impurity_decrease: 0.0,
            class_weights: None,
        }
    }
}

impl TrainConfig {
    /// Checks every field for range.
    ///
    /// # Errors
    /// Returns [`CartError::InvalidConfig`] naming the first bad field.
    pub fn validate(&self) -> Result<(), CartError> {
        let bad = |reason: &str| {
            Err(CartError::InvalidConfig {
                reason: reason.to_owned(),
            })
        };
        if self.min_samples_split < 2 {
            return bad("min_samples_split must be at least 2");
        }
        if self.min_samples_leaf < 1 {
            return bad("min_samples_leaf must be at least 1");
        }
        if !(self.min_impurity_decrease.is_finite() && self.min_impurity_decrease >= 0.0) {
            return bad("min_impurity_decrease must be finite and not negative");
        }
        if let Some(weights) = &self.class_weights {
            if weights.is_empty() {
                return bad("class_weights must not be empty");
            }
            if weights.iter().any(|w| !(w.is_finite() && *w > 0.0)) {
                return bad("class_weights must be positive and finite");
            }
        }
        Ok(())
    }
}
