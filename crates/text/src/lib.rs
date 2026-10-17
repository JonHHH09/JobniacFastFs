//! Java and Unicode text semantics for Jobniac (UTF-16, whitespace, case, segmentation).
//!
//! Stored text follows exact Java string rules (UTF-16 lengths, whitespace, lowercasing) and
//! Unicode character-class rules ([`str_rules`]). These helpers implement both, pinned by golden
//! vectors. Never substitute `str::trim`, `char::is_whitespace`, `str::lines` or
//! `split_whitespace` where a Java or Unicode rule from this crate applies.
#![forbid(unsafe_code)]

pub mod case;
pub mod html;
pub mod java;
pub mod json;
pub mod search;
pub mod sentences;
pub mod str_rules;
