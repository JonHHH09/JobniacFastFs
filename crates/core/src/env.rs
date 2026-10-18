//! An immutable snapshot of the process environment, read once at startup.
//!
//! Configuration loaders take an [`Environment`] instead of reading the process environment, so
//! they are pure and testable. Semantics:
//! - a missing variable and an empty value are different;
//! - on Windows, names are case-insensitive;
//! - values that are not valid Unicode are kept as present-but-invalid, so a
//!   loader treats them as malformed rather than as missing.

use std::collections::{BTreeMap, BTreeSet};

/// One looked-up value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvValue<'a> {
    Missing,
    Valid(&'a str),
    /// Present, but not valid Unicode.
    Invalid,
}

/// Immutable environment snapshot.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Environment {
    values: BTreeMap<String, String>,
    invalid: BTreeSet<String>,
    case_insensitive: bool,
}

/// The snapshot holds secrets (database password, HMAC secret, API keys), so its debug form shows
/// counts and flags only, never a name or a value.
impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("variables", &self.values.len())
            .field("invalid", &self.invalid.len())
            .field("case_insensitive", &self.case_insensitive)
            .finish()
    }
}

impl Environment {
    /// Build a snapshot from valid pairs (later duplicates win). Names are case-sensitive.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            values: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            invalid: BTreeSet::new(),
            case_insensitive: false,
        }
    }

    /// Build a snapshot from raw OS pairs. `case_insensitive` should be `cfg!(windows)`.
    pub fn from_os_pairs<I>(pairs: I, case_insensitive: bool) -> Self
    where
        I: IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    {
        let mut environment = Self {
            case_insensitive,
            ..Self::default()
        };
        for (key, value) in pairs {
            let Some(key) = key.to_str().map(|k| environment.normalize(k)) else {
                continue;
            };
            match value.into_string() {
                Ok(value) => {
                    environment.invalid.remove(&key);
                    environment.values.insert(key, value);
                }
                Err(_) => {
                    environment.values.remove(&key);
                    environment.invalid.insert(key);
                }
            }
        }
        environment
    }

    /// Mark a name as present with a non-Unicode value (for tests).
    pub fn with_invalid(mut self, key: &str) -> Self {
        let key = self.normalize(key);
        self.values.remove(&key);
        self.invalid.insert(key);
        self
    }

    fn normalize(&self, key: &str) -> String {
        if self.case_insensitive {
            key.to_uppercase()
        } else {
            key.to_owned()
        }
    }

    /// Look up a variable, distinguishing missing, valid and invalid values.
    pub fn lookup(&self, key: &str) -> EnvValue<'_> {
        let key = self.normalize(key);
        if let Some(value) = self.values.get(&key) {
            EnvValue::Valid(value)
        } else if self.invalid.contains(&key) {
            EnvValue::Invalid
        } else {
            EnvValue::Missing
        }
    }

    /// The value when present and valid Unicode; `None` when missing or invalid.
    pub fn get(&self, key: &str) -> Option<&str> {
        match self.lookup(key) {
            EnvValue::Valid(value) => Some(value),
            EnvValue::Missing | EnvValue::Invalid => None,
        }
    }

    /// Whether the variable is present at all (valid or invalid).
    pub fn contains(&self, key: &str) -> bool {
        !matches!(self.lookup(key), EnvValue::Missing)
    }

    /// True only for the exact value `1`.
    pub fn flag_enabled(&self, key: &str) -> bool {
        self.get(key) == Some("1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_shows_names_or_values() {
        let env = Environment::from_pairs([("JOBNIAC_EXAMPLE_SECRET", "synthetic-secret-value")])
            .with_invalid("BAD_NAME");
        let text = format!("{env:?}");
        assert!(!text.contains("synthetic-secret-value"));
        assert!(!text.contains("JOBNIAC_EXAMPLE_SECRET"));
        assert!(!text.contains("BAD_NAME"));
        assert!(text.contains("variables: 1"));
    }

    #[test]
    fn missing_empty_and_invalid_are_distinct() {
        let env = Environment::from_pairs([("A", ""), ("B", "x")]).with_invalid("C");
        assert_eq!(env.lookup("A"), EnvValue::Valid(""));
        assert_eq!(env.lookup("B"), EnvValue::Valid("x"));
        assert_eq!(env.lookup("C"), EnvValue::Invalid);
        assert_eq!(env.lookup("D"), EnvValue::Missing);
        assert!(env.contains("C"));
        assert_eq!(env.get("C"), None);
    }

    #[test]
    fn flags_require_the_exact_value_one() {
        let env = Environment::from_pairs([("ON", "1"), ("TRUE", "true"), ("SPACE", " 1")]);
        assert!(env.flag_enabled("ON"));
        assert!(!env.flag_enabled("TRUE"));
        assert!(!env.flag_enabled("SPACE"));
        assert!(!env.flag_enabled("MISSING"));
    }

    #[test]
    fn case_insensitive_snapshot_matches_any_case() {
        let env = Environment::from_os_pairs(
            [("Jobniac_Example_Setting".into(), "127.0.0.1".into())],
            true,
        );
        assert_eq!(env.get("JOBNIAC_EXAMPLE_SETTING"), Some("127.0.0.1"));
        assert_eq!(env.get("jobniac_example_setting"), Some("127.0.0.1"));
    }
}
