//! French translation of resume text: the translator ([`french`]), its case rules ([`fold`]) and the
//! glossary, built-in and user-extended ([`glossary`]).
//!
//! The built-in tables are generic vocabulary only; personal phrases come from the user glossary.

pub mod fold;
pub mod french;
pub mod glossary;

pub use fold::{preserve_case, unicode_fold};
pub use french::{FrenchTranslator, PreparedProtectedTerms};
pub use glossary::{
    BUILTIN_LABELS, BUILTIN_PHRASES, BUILTIN_WORDS, Glossary, USER_GLOSSARY_FORMAT,
    USER_GLOSSARY_MAX_ENTRIES, USER_GLOSSARY_MAX_TEXT_UNITS, USER_GLOSSARY_VERSION, UserGlossary,
};
