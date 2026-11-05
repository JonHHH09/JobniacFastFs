//! Line-level reading of resumes and cover letters: the line model, reading order, locale rules,
//! numeric line features, line-role labels and a seeded synthetic labelled corpus.
//!
//! This is the groundwork of a line-role classifier. Nothing here changes what
//! `DeterministicProfileTextExtractor::extract_profile` returns, and nothing in production calls it
//! yet; it is `pub` so that tests, a later trainer and the extractor can use it. The module is
//! pure: no IO, no database, deterministic.
//!
//! | Module | Content |
//! |---|---|
//! | [`model`] | [`TextLine`], [`LineGeometry`], [`lines_from_text`] |
//! | [`order`] | reading order of geometry lines (column detection) and wrapped-fragment joining |
//! | [`document`] | [`LineDocument`]: ordered lines, columns and the locale hint |
//! | [`dates`] | dates and ranges in English, French, German and Albanian, with day/month ambiguity |
//! | [`locale`] | [`LocaleHint`], phones, postal codes, US states and Canadian provinces |
//! | [`lexicon`] | the per-language lexicon tables: headings, letter markers, suffixes, degrees, titles, months |
//! | [`features`] | [`FEATURE_NAMES`] and [`line_features`]; the feature table is in its module docs |
//! | [`labels`] | [`ResumeLineRole`], [`LetterLineRole`] |
//! | `corpus` | the synthetic labelled corpus (hidden from the docs) |
//!
//! # Pipeline
//!
//! `LineDocument::from_text(text)` or `LineDocument::from_lines(geometry_lines)` puts the lines in
//! reading order and reads the locale; `line_features(&document)` gives one vector of
//! [`FEATURE_COUNT`] values per line. Geometry is optional per line: without it the geometry features
//! are neutral and `has_geometry` is 0.0.
//!
//! # Ambiguity
//!
//! A numeric date such as `03/04/2020` is read day-first for [`LocaleHint::Eu`], month-first for
//! [`LocaleHint::Us`], and refused ([`dates::DateOutcome::Ambiguous`]) for [`LocaleHint::Ca`] and
//! [`LocaleHint::Unknown`] when both parts are at most 12 and differ. `DD.MM.YYYY` is always
//! day-first and an unambiguous part above 12 decides regardless of the hint.

pub mod dates;
pub mod document;
pub mod features;
pub mod labels;
pub mod lexicon;
pub mod locale;
pub mod model;
pub mod order;

#[doc(hidden)]
pub mod corpus;

pub use dates::{
    DateOutcome, DateRange, PartialDate, RangeEnd, RangeOutcome, parse_date_range, parse_date_text,
};
pub use document::LineDocument;
pub use features::{FEATURE_COUNT, FEATURE_NAMES, feature_index, line_features};
pub use labels::{LetterLineRole, LineLabel, ResumeLineRole};
pub use locale::{LocaleHint, detect_locale};
pub use model::{LineGeometry, TextLine, lines_from_text};
pub use order::{ReadingOrder, read_lines};
