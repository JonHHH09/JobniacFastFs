//! Validation and row decoding of the document tables (SQLite).
//!
//! Stored rows are re-validated on every read: a value that does not match its bounds is corrupt
//! data and maps to the unavailable error, like every other database failure. Input that fails
//! the same rules maps to a domain-value error.

use jobniac_core::clock::{format_instant, parse_instant};
use jobniac_core::documents::{
    DocumentMetadata, ExtractionRecord, MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT,
    PageText,
};
use jobniac_core::errors::AppError;
use jobniac_text::java::utf16_length;
use jobniac_text::str_rules::strip_text;
use rusqlite::Row;
use rusqlite::types::FromSql;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::sqlite::codec::{Id, Instant};
use crate::sqlite::profiles::unavailable;

/// Largest UTF-16 length of a stored character count (the `integer` range of the contract).
pub const MAX_INTEGER: i64 = 2_147_483_647;
/// Largest page projection JSON, in bytes, that is read back (`_MAX_PAGE_JSON_BYTES`).
pub const MAX_PAGE_JSON_BYTES: i32 = 4 * 1024 * 1024;
/// Largest file name, in bytes, read back from the database.
const MAX_FILE_NAME_BYTES: usize = 4096;
/// Largest media type, in bytes, read back from the database.
const MAX_MEDIA_TYPE_BYTES: usize = 255;
/// Largest extractor version, in bytes.
const MAX_EXTRACTOR_BYTES: usize = 255;

/// The domain-value message of invalid file input.
pub const FILE_INPUT_MESSAGE: &str = "Document metadata and content must be valid and consistent";
/// The domain-value message of invalid extraction input.
pub const EXTRACTION_INPUT_MESSAGE: &str = "Canonical PDF extraction must be valid and bounded";

/// Whether a non-blank string of at most `max_bytes` UTF-8 bytes.
fn bounded_string(value: &str, max_bytes: usize) -> bool {
    !strip_text(value).is_empty() && value.len() <= max_bytes
}

/// Checks the bounds of stored or input metadata.
pub fn metadata_is_valid(metadata: &DocumentMetadata) -> bool {
    if !bounded_string(&metadata.original_file_name, MAX_FILE_NAME_BYTES)
        || !bounded_string(&metadata.media_type, MAX_MEDIA_TYPE_BYTES)
    {
        return false;
    }
    let size_ok = metadata.byte_size > 0
        && u64::try_from(metadata.byte_size).is_ok_and(|size| size <= MAX_FILE_BYTES as u64);
    let digest_ok = metadata.sha256.len() == 64
        && metadata
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let times = parse_instant(&metadata.created_at)
        .and_then(|created| parse_instant(&metadata.updated_at).map(|updated| (created, updated)));
    size_ok && digest_ok && matches!(times, Ok((created, updated)) if updated >= created)
}

/// Checks a file to store against its metadata.
///
/// # Errors
///
/// A domain-value error when the metadata is out of bounds, or the byte size or SHA-256 do not
/// match the content.
pub fn validate_file_input(metadata: &DocumentMetadata, content: &[u8]) -> Result<(), AppError> {
    let consistent = metadata_is_valid(metadata)
        && i64::try_from(content.len()).ok() == Some(metadata.byte_size)
        && sha256_hex(content) == metadata.sha256;
    if consistent {
        Ok(())
    } else {
        Err(AppError::DomainValue(FILE_INPUT_MESSAGE.to_owned()))
    }
}

/// Lowercase hexadecimal SHA-256 of `content`.
pub fn sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Checks a canonical extraction against the bounds of the table.
pub fn extraction_is_valid(record: &ExtractionRecord) -> bool {
    if !bounded_string(&record.extractor, MAX_EXTRACTOR_BYTES) {
        return false;
    }
    let text_units = utf16_length(&record.extracted_text);
    let units = i64::try_from(text_units).unwrap_or(i64::MAX);
    if text_units > MAX_CHARACTERS_LIMIT {
        return false;
    }
    if !(units <= record.character_count && record.character_count <= MAX_INTEGER) {
        return false;
    }
    if !(1..=MAX_PAGE_COUNT).contains(&record.page_count) {
        return false;
    }
    if !record.truncated && record.character_count != units {
        return false;
    }
    if record.pages.len() > usize::try_from(MAX_PAGE_COUNT).unwrap_or(usize::MAX) {
        return false;
    }
    let mut total = 0usize;
    let mut previous = 0u32;
    for page in &record.pages {
        if !(previous < page.page_number && page.page_number <= record.page_count) {
            return false;
        }
        total += utf16_length(&page.text);
        previous = page.page_number;
    }
    if total > MAX_CHARACTERS_LIMIT {
        return false;
    }
    parse_instant(&record.created_at).is_ok()
}

/// Checks a canonical extraction before it is written.
///
/// # Errors
///
/// A domain-value error when the record is out of bounds.
pub fn validate_extraction_input(record: &ExtractionRecord) -> Result<(), AppError> {
    if extraction_is_valid(record) {
        Ok(())
    } else {
        Err(AppError::DomainValue(EXTRACTION_INPUT_MESSAGE.to_owned()))
    }
}

/// Reads one column, mapping a missing or undecodable value to the unavailable error.
pub fn column<T: FromSql>(row: &Row<'_>, name: &str) -> Result<T, AppError> {
    row.get::<_, T>(name).map_err(|_| unavailable())
}

/// Builds the metadata of a row and re-checks it.
pub fn metadata_from_row(row: &Row<'_>) -> Result<DocumentMetadata, AppError> {
    let created: Instant = column(row, "created_at")?;
    let updated: Instant = column(row, "updated_at")?;
    let metadata = DocumentMetadata {
        id: column::<Id>(row, "id")?.0,
        original_file_name: column::<Option<String>>(row, "original_file_name")?
            .ok_or_else(unavailable)?,
        media_type: column::<Option<String>>(row, "media_type")?.ok_or_else(unavailable)?,
        byte_size: column(row, "byte_size")?,
        sha256: column(row, "sha256")?,
        created_at: format_instant(created.0),
        updated_at: format_instant(updated.0),
    };
    if metadata_is_valid(&metadata) {
        Ok(metadata)
    } else {
        Err(unavailable())
    }
}

/// Verifies the bytes read from a blob row against its size and digest. Missing content, a size
/// mismatch or a digest mismatch are unavailable.
pub fn verified_content(
    content: Option<Vec<u8>>,
    byte_size: i64,
    sha256: &str,
) -> Result<Vec<u8>, AppError> {
    let content = content.ok_or_else(unavailable)?;
    let within = !content.is_empty() && content.len() <= MAX_FILE_BYTES;
    if within
        && i64::try_from(content.len()).ok() == Some(byte_size)
        && sha256_hex(&content) == sha256
    {
        Ok(content)
    } else {
        Err(unavailable())
    }
}

/// Builds the canonical extraction of a row and re-checks it.
pub fn extraction_from_row(row: &Row<'_>) -> Result<ExtractionRecord, AppError> {
    let created: Instant = column(row, "created_at")?;
    let page_count: i64 = column(row, "page_count")?;
    let character_count: i64 = column(row, "character_count")?;
    let projections: Value = column::<Option<String>>(row, "page_projections")?
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or_else(unavailable)?;
    let record = ExtractionRecord {
        id: column::<Id>(row, "id")?.0,
        file_id: column::<Id>(row, "file_id")?.0,
        extractor: column::<Option<String>>(row, "extractor")?.ok_or_else(unavailable)?,
        character_count,
        page_count: u32::try_from(page_count).map_err(|_| unavailable())?,
        truncated: column(row, "truncated")?,
        extracted_text: column::<Option<String>>(row, "extracted_text")?.ok_or_else(unavailable)?,
        pages: pages_from_json(&projections).ok_or_else(unavailable)?,
        created_at: format_instant(created.0),
    };
    if extraction_is_valid(&record) {
        Ok(record)
    } else {
        Err(unavailable())
    }
}

/// Reads the page projections of a row: an array of objects with exactly `pageNumber` and `text`.
fn pages_from_json(value: &Value) -> Option<Vec<PageText>> {
    value
        .as_array()?
        .iter()
        .map(|page| {
            let object: &Map<String, Value> = page.as_object()?;
            if object.len() != 2 {
                return None;
            }
            let page_number = u32::try_from(object.get("pageNumber")?.as_u64()?).ok()?;
            let text = object.get("text")?.as_str()?.to_owned();
            Some(PageText { page_number, text })
        })
        .collect()
}

/// The JSON array of page projections written to the JSON text column.
pub fn pages_to_json(pages: &[PageText]) -> Value {
    Value::Array(
        pages
            .iter()
            .map(|page| {
                let mut object = Map::new();
                object.insert("pageNumber".to_owned(), Value::from(page.page_number));
                object.insert("text".to_owned(), Value::from(page.text.clone()));
                Value::Object(object)
            })
            .collect(),
    )
}

/// Parses a stored instant for a write, mapping a failure to the domain-value error.
pub fn instant_for_write(text: &str) -> Result<OffsetDateTime, AppError> {
    parse_instant(text).map_err(|_| AppError::DomainValue(FILE_INPUT_MESSAGE.to_owned()))
}

#[cfg(test)]
mod tests {
    use jobniac_core::clock::format_instant;
    use time::Duration;
    use uuid::Uuid;

    use super::*;

    const CONTENT: &[u8] = b"%PDF-1.4 synthetic";

    fn instant(seconds: i64) -> String {
        format_instant(OffsetDateTime::UNIX_EPOCH + Duration::seconds(seconds))
    }

    fn metadata() -> DocumentMetadata {
        DocumentMetadata {
            id: Uuid::nil(),
            original_file_name: "resume.pdf".to_owned(),
            media_type: "application/pdf".to_owned(),
            byte_size: i64::try_from(CONTENT.len()).unwrap_or(0),
            sha256: sha256_hex(CONTENT),
            created_at: instant(0),
            updated_at: instant(1),
        }
    }

    fn extraction(text: &str, character_count: i64, truncated: bool) -> ExtractionRecord {
        ExtractionRecord {
            id: Uuid::nil(),
            file_id: Uuid::nil(),
            extractor: "pdfium-test".to_owned(),
            character_count,
            page_count: 2,
            truncated,
            extracted_text: text.to_owned(),
            pages: vec![
                PageText {
                    page_number: 1,
                    text: "one".to_owned(),
                },
                PageText {
                    page_number: 2,
                    text: "two".to_owned(),
                },
            ],
            created_at: instant(0),
        }
    }

    #[test]
    fn metadata_accepts_a_consistent_record() {
        assert!(metadata_is_valid(&metadata()));
    }

    #[test]
    fn metadata_size_bounds_are_one_byte_to_the_import_limit() {
        let mut low = metadata();
        low.byte_size = 0;
        assert!(!metadata_is_valid(&low));
        let mut high = metadata();
        high.byte_size = i64::try_from(MAX_FILE_BYTES).unwrap_or(i64::MAX);
        assert!(metadata_is_valid(&high));
        high.byte_size += 1;
        assert!(!metadata_is_valid(&high));
    }

    #[test]
    fn metadata_rejects_a_bad_digest_blank_or_oversize_names_and_reversed_times() {
        let mut upper = metadata();
        upper.sha256 = upper.sha256.to_uppercase();
        assert!(!metadata_is_valid(&upper));
        let mut blank = metadata();
        blank.original_file_name = "  \t ".to_owned();
        assert!(!metadata_is_valid(&blank));
        let mut long = metadata();
        long.original_file_name = "a".repeat(MAX_FILE_NAME_BYTES + 1);
        assert!(!metadata_is_valid(&long));
        let mut media = metadata();
        media.media_type = "a".repeat(MAX_MEDIA_TYPE_BYTES + 1);
        assert!(!metadata_is_valid(&media));
        let mut reversed = metadata();
        reversed.updated_at = instant(-1);
        assert!(!metadata_is_valid(&reversed));
    }

    #[test]
    fn file_input_must_match_its_size_and_digest() {
        assert!(validate_file_input(&metadata(), CONTENT).is_ok());
        let mut other = metadata();
        other.sha256 = sha256_hex(b"other");
        assert_eq!(
            validate_file_input(&other, CONTENT),
            Err(AppError::DomainValue(FILE_INPUT_MESSAGE.to_owned()))
        );
    }

    #[test]
    fn verified_content_refuses_missing_resized_or_altered_bytes() {
        let size = i64::try_from(CONTENT.len()).unwrap_or(0);
        let digest = sha256_hex(CONTENT);
        assert_eq!(
            verified_content(Some(CONTENT.to_vec()), size, &digest),
            Ok(CONTENT.to_vec())
        );
        assert!(verified_content(None, size, &digest).is_err());
        assert!(verified_content(Some(Vec::new()), 0, &sha256_hex(b"")).is_err());
        assert!(verified_content(Some(b"%PDF-1.4 changed".to_vec()), size, &digest).is_err());
        assert!(verified_content(Some(CONTENT.to_vec()), size + 1, &digest).is_err());
    }

    #[test]
    fn extraction_counts_utf16_units_for_supplementary_characters() {
        // One emoji is two UTF-16 units, so the canonical count must be 2, not the code point count.
        assert!(extraction_is_valid(&extraction("\u{1F600}", 2, false)));
        assert!(!extraction_is_valid(&extraction("\u{1F600}", 1, false)));
        assert!(!extraction_is_valid(&extraction("abc", 2, false)));
        // A truncated text may count more characters than it keeps.
        assert!(extraction_is_valid(&extraction("abc", 9, true)));
    }

    #[test]
    fn extraction_pages_must_be_ordered_and_within_the_page_count() {
        let mut descending = extraction("abc", 3, false);
        descending.pages.reverse();
        assert!(!extraction_is_valid(&descending));
        let mut beyond = extraction("abc", 3, false);
        beyond.pages[1].page_number = 3;
        assert!(!extraction_is_valid(&beyond));
        let mut zero_pages = extraction("abc", 3, false);
        zero_pages.page_count = 0;
        assert!(!extraction_is_valid(&zero_pages));
    }

    #[test]
    fn extraction_text_beyond_the_canonical_limit_is_refused() {
        let text = "a".repeat(MAX_CHARACTERS_LIMIT + 1);
        let count = i64::try_from(text.len()).unwrap_or(i64::MAX);
        assert!(!extraction_is_valid(&extraction(&text, count, true)));
        assert!(validate_extraction_input(&extraction("abc", 3, false)).is_ok());
        assert_eq!(
            validate_extraction_input(&extraction("abc", 2, false)),
            Err(AppError::DomainValue(EXTRACTION_INPUT_MESSAGE.to_owned()))
        );
    }

    #[test]
    fn page_json_round_trips_and_rejects_extra_keys() {
        let pages = vec![PageText {
            page_number: 1,
            text: "\u{1F600} text".to_owned(),
        }];
        assert_eq!(pages_from_json(&pages_to_json(&pages)), Some(pages));
        let extra = serde_json::json!([{"pageNumber": 1, "text": "a", "extra": true}]);
        assert_eq!(pages_from_json(&extra), None);
        let missing = serde_json::json!([{"pageNumber": 1}]);
        assert_eq!(pages_from_json(&missing), None);
    }

    #[test]
    fn sha256_matches_the_published_digest_of_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
