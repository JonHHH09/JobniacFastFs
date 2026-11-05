//! The single error type of the crate.

use std::error::Error;
use std::fmt;

/// Everything that can go wrong in this crate. No function in the crate panics on bad input.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CartError {
    /// A feature or class name is empty, too long or uses characters outside `[A-Za-z0-9_.-]`.
    InvalidName {
        /// What was being named (`"feature"` or `"class"`).
        kind: &'static str,
        /// The offending name.
        name: String,
    },
    /// Two features or two classes share a name.
    DuplicateName {
        /// What was being named (`"feature"` or `"class"`).
        kind: &'static str,
        /// The repeated name.
        name: String,
    },
    /// A dataset needs at least one feature.
    NoFeatures,
    /// A dataset needs at least one class.
    NoClasses,
    /// A feature vector has the wrong length.
    FeatureCountMismatch {
        /// Number of features the dataset or tree declares.
        expected: usize,
        /// Number of values supplied.
        actual: usize,
    },
    /// A feature value is NaN or infinite.
    NonFiniteFeature {
        /// Position of the offending value in the feature vector.
        index: usize,
    },
    /// A class label is not below the class count.
    LabelOutOfRange {
        /// The label supplied.
        label: usize,
        /// Number of classes declared.
        class_count: usize,
    },
    /// A row weight is not a positive finite number.
    InvalidWeight {
        /// The weight supplied.
        weight: f64,
    },
    /// A row index is not below the dataset length.
    RowOutOfRange {
        /// The index supplied.
        row: usize,
        /// Number of rows in the dataset.
        len: usize,
    },
    /// Training needs at least one row.
    EmptyDataset,
    /// A structural limit (features, classes, nodes) was exceeded.
    LimitExceeded {
        /// The quantity that is limited.
        what: &'static str,
        /// The maximum allowed.
        max: usize,
    },
    /// A [`crate::TrainConfig`] value is out of range.
    InvalidConfig {
        /// Human-readable reason.
        reason: String,
    },
    /// A function argument (fold count, test fraction, alpha) is out of range.
    InvalidArgument {
        /// Human-readable reason.
        reason: String,
    },
    /// Model text could not be parsed or failed validation.
    InvalidModel {
        /// 1-based line number, or 0 when the error is not tied to one line.
        line: usize,
        /// Human-readable reason.
        reason: String,
    },
}

impl fmt::Display for CartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { kind, name } => write!(f, "invalid {kind} name {name:?}"),
            Self::DuplicateName { kind, name } => write!(f, "duplicate {kind} name {name:?}"),
            Self::NoFeatures => write!(f, "at least one feature is required"),
            Self::NoClasses => write!(f, "at least one class is required"),
            Self::FeatureCountMismatch { expected, actual } => {
                write!(f, "expected {expected} features, got {actual}")
            }
            Self::NonFiniteFeature { index } => write!(f, "feature {index} is not finite"),
            Self::LabelOutOfRange { label, class_count } => {
                write!(f, "label {label} is out of range for {class_count} classes")
            }
            Self::InvalidWeight { weight } => {
                write!(f, "weight {weight} is not a positive finite number")
            }
            Self::RowOutOfRange { row, len } => {
                write!(f, "row {row} is out of range for {len} rows")
            }
            Self::EmptyDataset => write!(f, "the dataset has no rows"),
            Self::LimitExceeded { what, max } => write!(f, "too many {what} (maximum {max})"),
            Self::InvalidConfig { reason } => write!(f, "invalid training config: {reason}"),
            Self::InvalidArgument { reason } => write!(f, "invalid argument: {reason}"),
            Self::InvalidModel { line, reason } => {
                if *line == 0 {
                    write!(f, "invalid model: {reason}")
                } else {
                    write!(f, "invalid model (line {line}): {reason}")
                }
            }
        }
    }
}

impl Error for CartError {}
