//! Pure, safe parser of the XML embedded in Europass PDFs (`attachment.xml`).
//!
//! Two schemas are read: the HR-XML based v4 `Candidate` CV and the legacy v3.4 `SkillsPassport`
//! cover letter (`ECL`). A v3 CV (`ECV`) is recognised and refused as unsupported. The input is
//! untrusted (it comes out of an arbitrary PDF): it must be UTF-8, at most [`MAX_INPUT_BYTES`]
//! long, free of a DTD (so no entity expansion or external entity) and below [`MAX_NODES`] nodes;
//! text fields are capped (see `xml::SHORT_TEXT_LIMIT` and `xml::RICH_TEXT_LIMIT`). No IO happens
//! here. Detection is by root element local name and namespace, never by file name.

mod candidate;
mod model;
mod rich_text;
mod skills_passport;
mod xml;

use roxmltree::{Document, Node, ParsingOptions};

use crate::errors::{AppError, validation_error};

pub use model::{
    Address, Addressee, Cefr, Design, DocumentInfo, Education, Employment, EuropassCoverLetter,
    EuropassCv, EuropassDocument, InvalidDate, LanguageSkill, PartialDate, Period, Person, Phone,
    RichBlock, RichText, Salutation, Skill,
};
pub use xml::{MAX_ENTRIES, RICH_TEXT_LIMIT, SHORT_TEXT_LIMIT};

/// Message of every rejected Europass document; the `reason` detail says why.
pub const EUROPASS_INVALID_MESSAGE: &str = "Europass document is invalid";

/// Message of a recognised Europass document type that is not supported yet.
pub const EUROPASS_UNSUPPORTED_MESSAGE: &str = "Europass document type is not supported";

/// Largest accepted document.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// Most XML nodes (elements, text, comments) a document may hold.
pub const MAX_NODES: u32 = 20_000;

/// Deepest element nesting (the root is depth 1); the real documents are about 8 deep.
pub const MAX_DEPTH: usize = 64;

const TOO_LARGE: &str = "too_large";
const NOT_UTF8: &str = "not_utf8";
const DTD_NOT_ALLOWED: &str = "dtd_not_allowed";
const TOO_COMPLEX: &str = "too_complex";
const MALFORMED: &str = "malformed_xml";
const NOT_EUROPASS: &str = "not_europass";

/// Most `xmlns` occurrences in the raw bytes (real Europass files have about 5).
const MAX_XMLNS: usize = 64;

/// Most quoted attribute values in the raw bytes. Real files have fewer than 50.
const MAX_QUOTED_VALUES: usize = 2_048;

/// The schema family of a Europass document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EuropassSchema {
    /// HR-XML based `Candidate` root (CV, schema version 4).
    CandidateV4,
    /// Legacy `SkillsPassport` root (v3.x): cover letter or CV, by `DocumentType`.
    SkillsPassportV3,
}

pub(super) fn reject(reason: &str, field: Option<&str>) -> AppError {
    validation_error(EUROPASS_INVALID_MESSAGE, field, Some(reason)).into()
}

fn unsupported(reason: &str) -> AppError {
    validation_error(
        EUROPASS_UNSUPPORTED_MESSAGE,
        Some("documentType"),
        Some(reason),
    )
    .into()
}

/// Position of `needle` in `hay` at or after `from`, as an index into `hay`.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let rest = hay.get(from..)?;
    rest.windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// Linear pre-scan over the raw bytes with cheap upper bounds, run before the tree is built.
///
/// `roxmltree` is super-linear in the number of attributes and namespace declarations of an
/// element and in nesting depth with namespaces, so a file inside the byte and node caps could still
/// burn seconds. The scan rejects more than [`MAX_XMLNS`] `xmlns` occurrences, more than
/// [`MAX_QUOTED_VALUES`] quoted attribute values inside tags and nesting deeper than [`MAX_DEPTH`].
/// Comments, CDATA sections and processing instructions are skipped; a document that only looks
/// pathological is rejected too, which is acceptable.
fn is_too_complex(bytes: &[u8]) -> bool {
    if bytes
        .windows(5)
        .filter(|window| *window == b"xmlns")
        .count()
        > MAX_XMLNS
    {
        return true;
    }
    let (mut quoted, mut depth, mut i) = (0usize, 0usize, 0usize);
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            Some(b'/') => {
                depth = depth.saturating_sub(1);
                i = find(bytes, b">", i).map_or(bytes.len(), |end| end + 1);
            }
            Some(b'!') => {
                i = if bytes[i..].starts_with(b"<!--") {
                    find(bytes, b"-->", i + 4).map_or(bytes.len(), |end| end + 3)
                } else if bytes[i..].starts_with(b"<![CDATA[") {
                    find(bytes, b"]]>", i + 9).map_or(bytes.len(), |end| end + 3)
                } else {
                    find(bytes, b">", i).map_or(bytes.len(), |end| end + 1)
                };
            }
            Some(b'?') => {
                i = find(bytes, b"?>", i + 2).map_or(bytes.len(), |end| end + 2);
            }
            _ => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return true;
                }
                let mut quote: Option<u8> = None;
                let mut j = i + 1;
                while j < bytes.len() {
                    let byte = bytes[j];
                    match quote {
                        Some(open) if byte == open => quote = None,
                        Some(_) => {}
                        None if byte == b'"' || byte == b'\'' => {
                            quote = Some(byte);
                            quoted += 1;
                            if quoted > MAX_QUOTED_VALUES {
                                return true;
                            }
                        }
                        None if byte == b'>' => break,
                        None => {}
                    }
                    j += 1;
                }
                if quote.is_none() && j > i + 1 && bytes.get(j - 1) == Some(&b'/') {
                    depth -= 1;
                }
                i = j + 1;
            }
        }
    }
    false
}

/// Parse bytes into a tree with every limit applied, then run `read` on it.
fn with_document<T>(
    bytes: &[u8],
    read: impl FnOnce(&Document<'_>) -> Result<T, AppError>,
) -> Result<T, AppError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(reject(TOO_LARGE, None));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| reject(NOT_UTF8, None))?;
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    if is_too_complex(text.as_bytes()) {
        return Err(reject(TOO_COMPLEX, None));
    }
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_NODES,
        ..ParsingOptions::default()
    };
    let document = Document::parse_with_options(text, options).map_err(|error| {
        reject(
            match error {
                roxmltree::Error::DtdDetected => DTD_NOT_ALLOWED,
                roxmltree::Error::NodesLimitReached => TOO_COMPLEX,
                _ => MALFORMED,
            },
            None,
        )
    })?;
    // Bounded ancestor walks keep this linear in the node count.
    if document
        .descendants()
        .any(|node| node.ancestors().take(MAX_DEPTH + 2).count() > MAX_DEPTH + 1)
    {
        return Err(reject(TOO_COMPLEX, None));
    }
    read(&document)
}

fn schema_of(root: Node<'_, '_>) -> Option<EuropassSchema> {
    let name = root.tag_name();
    match (name.name(), name.namespace()) {
        ("Candidate", Some(candidate::EU)) => Some(EuropassSchema::CandidateV4),
        ("SkillsPassport", Some(skills_passport::NS)) => Some(EuropassSchema::SkillsPassportV3),
        _ => None,
    }
}

/// Which Europass schema the bytes hold, or `None` when they are not a readable Europass document
/// (too large, not UTF-8, not well-formed, DTD present, wrong root or namespace).
pub fn detect(bytes: &[u8]) -> Option<EuropassSchema> {
    with_document(bytes, |document| Ok(schema_of(document.root_element())))
        .ok()
        .flatten()
}

/// Parse a Europass XML document into the typed model.
///
/// Errors are `validation_error`s: [`EUROPASS_INVALID_MESSAGE`] with a `reason` detail
/// (`too_large`, `not_utf8`, `dtd_not_allowed`, `too_complex`, `malformed_xml`, `not_europass`),
/// or [`EUROPASS_UNSUPPORTED_MESSAGE`] for a v3 document that is not a cover letter
/// (`legacy_cv_not_supported` for `ECV`, `document_type_not_supported` else). A malformed date,
/// flag or CEFR level does not fail the document: the value is dropped and its path listed in
/// `invalid_fields`.
pub fn parse(bytes: &[u8]) -> Result<EuropassDocument, AppError> {
    with_document(bytes, |document| {
        let root = document.root_element();
        match schema_of(root) {
            None => Err(reject(NOT_EUROPASS, None)),
            Some(EuropassSchema::CandidateV4) => {
                Ok(EuropassDocument::Cv(Box::new(candidate::parse(root))))
            }
            Some(EuropassSchema::SkillsPassportV3) => {
                match skills_passport::document_type(root).as_deref() {
                    Some("ECL") => Ok(EuropassDocument::CoverLetter(Box::new(
                        skills_passport::parse_cover_letter(root),
                    ))),
                    Some("ECV") => Err(unsupported("legacy_cv_not_supported")),
                    _ => Err(unsupported("document_type_not_supported")),
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_dates_parse_in_all_precisions() {
        assert_eq!(
            PartialDate::parse("2023").map(|d| d.to_string()),
            Some("2023".into())
        );
        assert_eq!(
            PartialDate::parse("2023-03").map(|d| d.to_string()),
            Some("2023-03".into())
        );
        assert_eq!(
            PartialDate::parse("2023-03-15T10:00:00Z").map(|d| d.to_string()),
            Some("2023-03-15".into())
        );
        assert_eq!(
            PartialDate::parse(" 2024-02-29 ").map(|d| d.day()),
            Some(Some(29))
        );
    }

    #[test]
    fn partial_dates_drop_time_zone_suffixes() {
        for (text, expected) in [
            ("2023-03-15Z", "2023-03-15"),
            ("2023-03-15+01:00", "2023-03-15"),
            ("2023-03-15-05:30", "2023-03-15"),
            ("2023-03-15T10:00:00+02:00", "2023-03-15"),
            ("2023-03Z", "2023-03"),
            ("2023Z", "2023"),
        ] {
            assert_eq!(
                PartialDate::parse(text).map(|d| d.to_string()).as_deref(),
                Some(expected),
                "{text}"
            );
        }
        assert_eq!(PartialDate::parse("2023-03-15+1:00"), None);
    }

    #[test]
    fn partial_dates_reject_impossible_values() {
        for bad in [
            "",
            "23",
            "2023-13",
            "2023-00",
            "2023-02-29",
            "2023-04-31",
            "2023-3-1",
            "2023-03-15x",
            "2023-03-15T",
            "0000",
            "2023-03-15-01",
            "abcd",
            "２０２３",
        ] {
            assert_eq!(PartialDate::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn g_parts_parse_the_xsd_lexical_forms() {
        let date = PartialDate::from_g_parts(Some("2026"), Some("--11"), Some("---03"));
        assert_eq!(
            date.map(|d| d.map(|d| d.to_string())),
            Ok(Some("2026-11-03".into()))
        );
        let zoned = PartialDate::from_g_parts(Some("2026Z"), Some("--02+01:00"), Some("---28"));
        assert_eq!(
            zoned.map(|d| d.map(|d| d.to_string())),
            Ok(Some("2026-02-28".into()))
        );
        assert_eq!(
            PartialDate::from_g_parts(Some("2026"), None, None).map(|d| d.map(|d| d.to_string())),
            Ok(Some("2026".into()))
        );
        assert_eq!(PartialDate::from_g_parts(None, None, None), Ok(None));
    }

    #[test]
    fn g_parts_reject_malformed_or_inconsistent_values() {
        for (y, m, d) in [
            (Some("2026"), Some("11"), None),
            (Some("2026"), Some("--13"), None),
            (Some("2026"), None, Some("---03")),
            (Some("2026"), Some("--02"), Some("---30")),
            (Some("2026"), Some("--11"), Some("--03")),
            (None, Some("--11"), None),
            (Some("26"), None, None),
            (Some("2026Q"), None, None),
        ] {
            assert_eq!(
                PartialDate::from_g_parts(y, m, d),
                Err(InvalidDate),
                "{y:?} {m:?} {d:?}"
            );
        }
    }

    #[test]
    fn cefr_parses_all_levels_and_nothing_else() {
        for (text, level) in [
            ("A1", Cefr::A1),
            ("a2", Cefr::A2),
            (" B1 ", Cefr::B1),
            ("B2", Cefr::B2),
            ("C1", Cefr::C1),
            ("c2", Cefr::C2),
        ] {
            assert_eq!(Cefr::parse(text), Some(level));
            assert_eq!(Cefr::parse(level.as_str()), Some(level));
        }
        for bad in ["", "A", "D1", "A3", "B1+", "native"] {
            assert_eq!(Cefr::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn phones_assemble_dialing_code_and_number() {
        let phone = |dialing: Option<&str>, number: &str| Phone {
            use_code: None,
            country_dialing: dialing.map(str::to_owned),
            number: number.to_owned(),
            country_code: None,
        };
        assert_eq!(
            phone(Some("1"), "2025550147").international(),
            "+1 2025550147"
        );
        assert_eq!(
            phone(Some("+49"), "30 1234567").international(),
            "+49 30 1234567"
        );
        assert_eq!(
            phone(Some("39"), "0612345678").international(),
            "+39 0612345678"
        );
        assert_eq!(
            phone(Some("1"), "+12025550147").international(),
            "+12025550147"
        );
        assert_eq!(phone(None, "+12025550147").international(), "+12025550147");
        assert_eq!(phone(Some(" "), "0301234").international(), "0301234");
    }

    #[test]
    fn rich_text_plain_marks_bullets_and_gaps() {
        let text = RichText {
            markup: None,
            blocks: vec![
                RichBlock::Paragraph("Intro".into()),
                RichBlock::Blank,
                RichBlock::Bullet("one".into()),
            ],
        };
        assert_eq!(text.plain(), "Intro\n\n- one");
    }
}
