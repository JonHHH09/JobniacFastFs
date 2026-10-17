//! The translation glossary: the generic built-in tables and the optional user glossary file.
//!
//! The built-in tables hold only generic vocabulary: the label set and neutral
//! words and phrases. A user may extend them with a JSON document of the format described on
//! [`UserGlossary::parse`]. Only the parsed value crosses into core; the file is read by the
//! infrastructure layer, outside the repository.

use std::collections::{BTreeMap, HashSet};

use jobniac_text::java::{java_is_blank, java_strip, utf16_length};
use serde::Deserialize;
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::errors::{ApplicationError, validation_error};

/// The format tag a user glossary must carry in its `format` member.
pub const USER_GLOSSARY_FORMAT: &str = "jobniac-translation-glossary";

/// The only glossary document version this build reads.
pub const USER_GLOSSARY_VERSION: u32 = 1;

/// Most entries one section (`labels`, `phrases` or `words`) of a user glossary may hold.
pub const USER_GLOSSARY_MAX_ENTRIES: usize = 1_000;

/// Most UTF-16 units one label, phrase or word text of a user glossary may hold.
pub const USER_GLOSSARY_MAX_TEXT_UNITS: usize = 512;

/// The largest glossary document [`UserGlossary::parse`] reads, in bytes: three sections of
/// [`USER_GLOSSARY_MAX_ENTRIES`] entries of two texts at the text limit, with escapes, plus slack.
pub const USER_GLOSSARY_MAX_BYTES: usize = 20 * 1024 * 1024;

/// Generic label set: the source label (lower case) and its French form.
pub const BUILTIN_LABELS: &[(&str, &str)] = &[
    ("contact", "Coordonnées"),
    ("links", "Liens"),
    ("email", "Courriel"),
    ("phone", "Téléphone"),
    ("location", "Lieu"),
    ("address", "Adresse"),
    ("website", "Site Web"),
    ("portfolio", "Portfolio"),
    ("github", "GitHub"),
    ("linkedin", "LinkedIn"),
    ("backend", "Développement backend"),
    ("frontend", "Développement frontend"),
    ("frontend / apps", "Développement frontend / Applications"),
    ("cloud / ops", "Infonuagique / Exploitation"),
    ("security", "Sécurité"),
    ("systems / tools", "Systèmes / Outils"),
    ("general", "Général"),
    ("fluent", "Courant"),
    ("native", "Langue maternelle"),
    ("professional", "Professionnel"),
    ("intermediate", "Intermédiaire"),
    ("beginner", "Débutant"),
];

/// Generic whole phrases, applied in this order (a later phrase sees the output of an earlier one).
pub const BUILTIN_PHRASES: &[(&str, &str)] = &[
    ("Current Role", "Poste actuel"),
    ("Past Role", "Poste précédent"),
    ("Undated Role", "Poste sans date"),
    ("Full-Stack Developer", "Développeur full-stack"),
    ("Front-End Developer", "Développeur frontend"),
    ("Software Developer", "Développeur logiciel"),
    ("Software engineering", "Génie logiciel"),
    ("Computer Science", "Informatique"),
    ("English", "Anglais"),
    ("French", "Français"),
    ("German", "Allemand"),
    ("Remote", "À distance"),
];

/// Generic word glossary: the lower-case ASCII word and its French form.
pub const BUILTIN_WORDS: &[(&str, &str)] = &[
    ("built", "élaboré"),
    ("build", "développer"),
    ("developed", "développé"),
    ("develop", "développer"),
    ("implemented", "mis en place"),
    ("integrated", "intégré"),
    ("managed", "géré"),
    ("supported", "soutenu"),
    ("automated", "automatisé"),
    ("improved", "amélioré"),
    ("added", "ajouté"),
    ("created", "créé"),
    ("delivered", "livré"),
    ("i", "je"),
    ("systems", "systèmes"),
    ("system", "système"),
    ("application", "application"),
    ("applications", "applications"),
    ("software", "logiciel"),
    ("developer", "développeur"),
    ("engineering", "génie"),
    ("education", "formation"),
    ("languages", "langues"),
    ("language", "langue"),
    ("present", "présent"),
    ("role", "poste"),
    ("degree", "diplôme"),
    ("institution", "établissement"),
    ("canada", "Canada"),
    ("english", "anglais"),
    ("french", "français"),
    ("german", "allemand"),
    ("remote", "à distance"),
    ("fluent", "courant"),
    ("professional", "professionnel"),
    ("intermediate", "intermédiaire"),
    ("beginner", "débutant"),
];

/// A complete translation glossary: labels, phrases and words, ready for the translator.
///
/// Label and word keys are lower case. `phrases` keeps its application order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glossary {
    /// Label map: lower-case source label to its French label.
    pub labels: BTreeMap<String, String>,
    /// Ordered phrase pairs: source phrase and its French translation.
    pub phrases: Vec<(String, String)>,
    /// Word map: lower-case ASCII word to its French word.
    pub words: BTreeMap<String, String>,
}

impl Glossary {
    /// The generic built-in glossary.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            labels: pairs_to_map(BUILTIN_LABELS),
            phrases: BUILTIN_PHRASES
                .iter()
                .map(|(source, target)| ((*source).to_owned(), (*target).to_owned()))
                .collect(),
            words: pairs_to_map(BUILTIN_WORDS),
        }
    }

    /// The glossary with a user glossary applied.
    ///
    /// A user label or word replaces the built-in entry with the same key. User phrases are tried
    /// before the built-in phrases, in the order of the user document.
    #[must_use]
    pub fn merged_with(&self, user: &UserGlossary) -> Self {
        let mut merged = self.clone();
        for (key, value) in &user.labels {
            merged.labels.insert(key.clone(), value.clone());
        }
        for (key, value) in &user.words {
            merged.words.insert(key.clone(), value.clone());
        }
        let mut phrases = user.phrases.clone();
        phrases.append(&mut merged.phrases);
        merged.phrases = phrases;
        merged
    }
}

fn pairs_to_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// A validated user glossary, already normalized: label and word keys are lower case, and every
/// text is non-blank and has no surrounding whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserGlossary {
    /// Labels, keyed by the stripped, lower-cased source label.
    pub labels: Vec<(String, String)>,
    /// Phrases in document order.
    pub phrases: Vec<(String, String)>,
    /// Words, keyed by the lower-cased ASCII word.
    pub words: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGlossary {
    format: String,
    version: u32,
    #[serde(default)]
    labels: Option<PairList>,
    #[serde(default)]
    phrases: Option<PhraseList>,
    #[serde(default)]
    words: Option<PairList>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPhrase {
    source: String,
    target: String,
}

/// A JSON array of phrases that stops collecting once it is past the entry limit (the count check
/// then refuses it), so an oversized array is not held in memory.
struct PhraseList(Vec<RawPhrase>);

impl<'de> Deserialize<'de> for PhraseList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PhraseVisitor;
        impl<'de> Visitor<'de> for PhraseVisitor {
            type Value = PhraseList;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an array of phrases")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PhraseList, A::Error> {
                let mut phrases = Vec::new();
                while let Some(phrase) = seq.next_element::<RawPhrase>()? {
                    phrases.push(phrase);
                    if phrases.len() > USER_GLOSSARY_MAX_ENTRIES {
                        while seq.next_element::<IgnoredAny>()?.is_some() {}
                        break;
                    }
                }
                Ok(PhraseList(phrases))
            }
        }
        deserializer.deserialize_seq(PhraseVisitor)
    }
}

/// A JSON object read as an ordered list of string pairs, so that repeated keys are seen (and
/// refused after normalization) instead of silently replacing each other.
#[derive(Debug)]
struct PairList(Vec<(String, String)>);

impl<'de> Deserialize<'de> for PairList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PairVisitor;
        impl<'de> Visitor<'de> for PairVisitor {
            type Value = PairList;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an object of strings")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PairList, A::Error> {
                let mut pairs = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    pairs.push((key, value));
                    if pairs.len() > USER_GLOSSARY_MAX_ENTRIES {
                        // Over the limit: the count check refuses it, so stop collecting.
                        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                        break;
                    }
                }
                Ok(PairList(pairs))
            }
        }
        deserializer.deserialize_map(PairVisitor)
    }
}

impl UserGlossary {
    /// Parses and validates a user glossary document.
    ///
    /// The document is one JSON object with these members. `format` must be
    /// `jobniac-translation-glossary` and `version` must be `1`. The optional members are
    /// `labels` (object of strings: source label to French label), `phrases` (array of objects
    /// with `source` and `target` strings) and `words` (object of strings: ASCII word to French
    /// word). Unknown members are refused.
    ///
    /// Limits: at most [`USER_GLOSSARY_MAX_ENTRIES`] entries per section and at most
    /// [`USER_GLOSSARY_MAX_TEXT_UNITS`] UTF-16 units per text. Every text is non-blank and has no
    /// surrounding whitespace. A word must start with an ASCII letter and continue with ASCII
    /// letters or one of `+ . / -`, the token shape the translator matches. Keys that are equal
    /// after normalization are refused.
    ///
    /// # Errors
    ///
    /// Returns a `validation_error` with message `Invalid translation glossary`. The `field` detail
    /// names the section (`glossary`, `labels`, `phrases` or `words`) and the `reason` detail
    /// states the rule. No value of the document is echoed.
    pub fn parse(text: &str) -> Result<Self, ApplicationError> {
        if text.len() > USER_GLOSSARY_MAX_BYTES {
            return Err(glossary_error("glossary", "exceeds the size limit"));
        }
        let raw: RawGlossary = serde_json::from_str(text)
            .map_err(|_| glossary_error("glossary", "must be a valid JSON glossary document"))?;
        if raw.format != USER_GLOSSARY_FORMAT || raw.version != USER_GLOSSARY_VERSION {
            return Err(glossary_error(
                "glossary",
                "must be a version 1 translation glossary document",
            ));
        }
        let labels = normalize_pairs(raw.labels.map(|list| list.0), "labels", true)?;
        let words = normalize_pairs(raw.words.map(|list| list.0), "words", false)?;
        let phrases = raw.phrases.map(|list| list.0).unwrap_or_default();
        if phrases.len() > USER_GLOSSARY_MAX_ENTRIES {
            return Err(glossary_error("phrases", "exceeds the entry limit"));
        }
        let mut normalized_phrases = Vec::with_capacity(phrases.len());
        for phrase in phrases {
            check_text(&phrase.source, "phrases")?;
            check_text(&phrase.target, "phrases")?;
            normalized_phrases.push((phrase.source, phrase.target));
        }
        Ok(Self {
            labels,
            phrases: normalized_phrases,
            words,
        })
    }
}

fn glossary_error(field: &str, reason: &str) -> ApplicationError {
    validation_error("Invalid translation glossary", Some(field), Some(reason))
}

/// Checks one stripped, non-blank text against the length limit.
fn check_text(value: &str, field: &str) -> Result<(), ApplicationError> {
    if java_is_blank(value) {
        return Err(glossary_error(field, "must not be blank"));
    }
    if java_strip(value) != value {
        return Err(glossary_error(
            field,
            "must not have surrounding whitespace",
        ));
    }
    if utf16_length(value) > USER_GLOSSARY_MAX_TEXT_UNITS {
        return Err(glossary_error(field, "exceeds the text length limit"));
    }
    Ok(())
}

/// Normalizes one pair section: the key is stripped and lower-cased (labels) or lower-cased only
/// (words, which must also be ASCII tokens), the value is checked, and duplicate keys are refused.
fn normalize_pairs(
    pairs: Option<Vec<(String, String)>>,
    field: &str,
    labels: bool,
) -> Result<Vec<(String, String)>, ApplicationError> {
    let pairs = pairs.unwrap_or_default();
    if pairs.len() > USER_GLOSSARY_MAX_ENTRIES {
        return Err(glossary_error(field, "exceeds the entry limit"));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(pairs.len());
    for (key, value) in pairs {
        if !labels && !is_word_token(&key) {
            return Err(glossary_error(
                field,
                "must be an ASCII word starting with a letter",
            ));
        }
        check_text(&key, field)?;
        check_text(&value, field)?;
        let key = if labels {
            key.to_lowercase()
        } else {
            key.to_ascii_lowercase()
        };
        if !seen.insert(key.clone()) {
            return Err(glossary_error(field, "is duplicated"));
        }
        normalized.push((key, value));
    }
    Ok(normalized)
}

/// True for the token shape the translator matches: `[A-Za-z][A-Za-z+./-]*`.
fn is_word_token(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic())
        && characters.all(|c| c.is_ascii_alphabetic() || matches!(c, '+' | '.' | '/' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tables_have_the_generic_sizes() {
        let glossary = Glossary::builtin();
        assert_eq!(glossary.labels.len(), 22);
        assert_eq!(glossary.phrases.len(), 12);
        assert_eq!(glossary.words.len(), 37);
    }

    #[test]
    fn parse_accepts_a_minimal_document() {
        let user = UserGlossary::parse(r#"{"format":"jobniac-translation-glossary","version":1}"#)
            .expect("minimal document");
        assert_eq!(user, UserGlossary::default());
    }

    #[test]
    fn parse_refuses_wrong_format_and_unknown_members() {
        assert!(UserGlossary::parse(r#"{"format":"other","version":1}"#).is_err());
        assert!(
            UserGlossary::parse(r#"{"format":"jobniac-translation-glossary","version":2}"#)
                .is_err()
        );
        assert!(
            UserGlossary::parse(
                r#"{"format":"jobniac-translation-glossary","version":1,"extra":{}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn parse_refuses_duplicate_keys_after_normalization() {
        let text = r#"{"format":"jobniac-translation-glossary","version":1,
            "words":{"Tool":"outil","tool":"outil"}}"#;
        let error = UserGlossary::parse(text).expect_err("duplicate");
        assert_eq!(
            error.details().get("field").map(String::as_str),
            Some("words")
        );
        assert_eq!(
            error.details().get("reason").map(String::as_str),
            Some("is duplicated")
        );
    }

    #[test]
    fn parse_refuses_raw_duplicate_keys() {
        let text = r#"{"format":"jobniac-translation-glossary","version":1,
            "words":{"tool":"outil","tool":"autre"}}"#;
        assert!(UserGlossary::parse(text).is_err());
    }

    #[test]
    fn parse_refuses_a_word_key_over_the_text_limit() {
        let document = |length: usize| {
            format!(
                r#"{{"format":"jobniac-translation-glossary","version":1,"words":{{"{}":"x"}}}}"#,
                "a".repeat(length)
            )
        };
        assert!(UserGlossary::parse(&document(USER_GLOSSARY_MAX_TEXT_UNITS)).is_ok());
        let error = UserGlossary::parse(&document(USER_GLOSSARY_MAX_TEXT_UNITS + 1))
            .expect_err("513-unit word key");
        assert_eq!(
            error.details().get("reason").map(String::as_str),
            Some("exceeds the text length limit")
        );
    }

    #[test]
    fn parse_refuses_over_limit_sections_and_oversized_text() {
        let entries = (0..=USER_GLOSSARY_MAX_ENTRIES)
            .map(|index| format!(r#""k{index}":"v""#))
            .collect::<Vec<_>>()
            .join(",");
        let labels = format!(
            r#"{{"format":"jobniac-translation-glossary","version":1,"labels":{{{entries}}}}}"#
        );
        let error = UserGlossary::parse(&labels).expect_err("too many labels");
        assert_eq!(
            error.details().get("reason").map(String::as_str),
            Some("exceeds the entry limit")
        );
        let phrases = (0..=USER_GLOSSARY_MAX_ENTRIES)
            .map(|_| r#"{"source":"a","target":"b"}"#)
            .collect::<Vec<_>>()
            .join(",");
        let phrases = format!(
            r#"{{"format":"jobniac-translation-glossary","version":1,"phrases":[{phrases}]}}"#
        );
        let error = UserGlossary::parse(&phrases).expect_err("too many phrases");
        assert_eq!(
            error.details().get("reason").map(String::as_str),
            Some("exceeds the entry limit")
        );
        let huge = " ".repeat(USER_GLOSSARY_MAX_BYTES + 1);
        let error = UserGlossary::parse(&huge).expect_err("over the byte cap");
        assert_eq!(
            error.details().get("reason").map(String::as_str),
            Some("exceeds the size limit")
        );
    }

    #[test]
    fn parse_refuses_bad_words_and_whitespace() {
        let bad_word =
            r#"{"format":"jobniac-translation-glossary","version":1,"words":{"1tool":"x"}}"#;
        assert!(UserGlossary::parse(bad_word).is_err());
        let padded = r#"{"format":"jobniac-translation-glossary","version":1,"phrases":[{"source":" a ","target":"b"}]}"#;
        assert!(UserGlossary::parse(padded).is_err());
        let blank = r#"{"format":"jobniac-translation-glossary","version":1,"labels":{"x":" "}}"#;
        assert!(UserGlossary::parse(blank).is_err());
    }

    #[test]
    fn merge_overrides_labels_and_words_and_prepends_phrases() {
        let user = UserGlossary {
            labels: vec![("general".to_owned(), "Généraliste".to_owned())],
            phrases: vec![("Remote".to_owned(), "Télétravail".to_owned())],
            words: vec![("remote".to_owned(), "télétravail".to_owned())],
        };
        let merged = Glossary::builtin().merged_with(&user);
        assert_eq!(
            merged.labels.get("general").map(String::as_str),
            Some("Généraliste")
        );
        assert_eq!(
            merged.words.get("remote").map(String::as_str),
            Some("télétravail")
        );
        assert_eq!(
            merged.phrases[0],
            ("Remote".to_owned(), "Télétravail".to_owned())
        );
        assert_eq!(merged.phrases.len(), 13);
    }
}
