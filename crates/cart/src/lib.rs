//! A dependency-free CART (Classification and Regression Trees, classification only) library.
//!
//! The crate is generic: it knows nothing about what the features mean. Build a [`Dataset`], call
//! [`train`] with a [`TrainConfig`], optionally prune with [`cost_complexity_path`], [`prune`] and
//! [`select_alpha`], score with [`ConfusionMatrix`], and persist with [`Tree::to_text`] and
//! [`Tree::from_text`].
//!
//! Everything is deterministic: the same inputs give bit-identical trees and text on every
//! platform (no threads, no hash-map iteration, fixed summation order, built-in [`SplitMix64`]
//! random numbers and a portable `log2`).
#![forbid(unsafe_code)]

mod config;
mod dataset;
mod error;
mod metrics;
mod prune;
mod rng;
mod select;
mod text;
mod train;
mod tree;

pub use config::{Criterion, TrainConfig};
pub use dataset::{Dataset, MAX_CLASSES, MAX_FEATURES, MAX_NAME_LEN};
pub use error::CartError;
pub use metrics::ConfusionMatrix;
pub use prune::{cost_complexity_path, prune};
pub use rng::SplitMix64;
pub use select::{AlphaScore, AlphaSelection, select_alpha};
pub use text::{HEADER, MAX_MODEL_BYTES};
pub use train::train;
pub use tree::{MAX_NODES, Node, Split, Tree};
