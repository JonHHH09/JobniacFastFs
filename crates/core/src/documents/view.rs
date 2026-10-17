//! Pure rules of the document use cases: file names, PDF content checks, display views and the
//! request parsing, in its validation order.

use jobniac_text::java::{truncate_utf16, utf16_length};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::errors::{AppError, validation_error};
use crate::ids::{UuidRefusal, admit_uuid};

use super::model::{
    DEFAULT_MAX_CHARACTERS, EXTRACTION_VALIDATION_MESSAGE, ExtractionOptions, FALLBACK_FILE_NAME,
    MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT, PdfText, STORAGE_VALIDATION_MESSAGE,
};

/// The reason of a non-PDF or otherwise refused request body.
const REQUEST_NULL_REASON: &str = "must not be null";

/// Whether a zero-based page index names a page of a document with `page_count` pages: the count
/// is between 1 and [`MAX_PAGE_COUNT`], and the index is below the count.
#[must_use]
pub const fn page_index_in_range(index: u32, page_count: u32) -> bool {
    page_count >= 1 && page_count <= MAX_PAGE_COUNT && index < page_count
}

/// Reduces a file name to its last component, treating `\` and `/` both as separators.
/// An empty result becomes `stored.pdf`.
#[must_use]
pub fn safe_basename(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let last = normalized.rsplit('/').next().unwrap_or_default();
    if last.is_empty() {
        FALLBACK_FILE_NAME.to_owned()
    } else {
        last.to_owned()
    }
}

/// Builds the validation error of a document use case.
#[must_use]
pub fn document_validation(message: &str, field: &str, reason: &str) -> AppError {
    AppError::from(validation_error(message, Some(field), Some(reason)))
}

/// Checks the bytes of a PDF in order: empty (storage only), size, then the header.
///
/// # Errors
///
/// A validation error on the `path` field with the reason of the first failing rule.
pub fn validate_pdf_content(content: &[u8], storage: bool) -> Result<(), AppError> {
    let message = if storage {
        STORAGE_VALIDATION_MESSAGE
    } else {
        EXTRACTION_VALIDATION_MESSAGE
    };
    if storage && content.is_empty() {
        return Err(document_validation(
            message,
            "path",
            "file must not be empty",
        ));
    }
    if content.len() > MAX_FILE_BYTES {
        let prefix = if storage { "file" } else { "PDF file" };
        return Err(document_validation(
            message,
            "path",
            &format!("{prefix} size exceeds limit of {MAX_FILE_BYTES} bytes"),
        ));
    }
    if !content.starts_with(b"%PDF-") {
        return Err(document_validation(
            message,
            "path",
            "file must be a PDF document",
        ));
    }
    Ok(())
}

/// Applies the caller display budget to a canonical extraction without changing the canonical
/// value. Pages are cut in order; each page takes what remains.
#[must_use]
pub fn extraction_view(canonical: &PdfText, maximum: usize, include_pages: bool) -> PdfText {
    let mut pages = Vec::new();
    if include_pages {
        let mut remaining = maximum;
        for page in &canonical.pages {
            if remaining == 0 {
                break;
            }
            let text = truncate_utf16(&page.text, remaining).to_owned();
            remaining = remaining.saturating_sub(utf16_length(&text));
            pages.push(super::model::PageText {
                page_number: page.page_number,
                text,
            });
        }
    }
    PdfText {
        file_name: safe_basename(&canonical.file_name),
        page_count: canonical.page_count,
        character_count: canonical.character_count,
        truncated: canonical.character_count > i64::try_from(maximum).unwrap_or(i64::MAX),
        text: truncate_utf16(&canonical.text, maximum).to_owned(),
        pages,
    }
}

/// Reads the request body as an object.
///
/// # Errors
///
/// A validation error on the `request` field when the value is not an object.
pub fn request_object<'a>(
    value: &'a Value,
    message: &str,
) -> Result<&'a Map<String, Value>, AppError> {
    value
        .as_object()
        .ok_or_else(|| document_validation(message, "request", REQUEST_NULL_REASON))
}

/// Reads an optional boolean. Absent or null takes the default.
///
/// # Errors
///
/// A validation error on `field` when the value is present and not a boolean.
pub fn optional_boolean(
    request: &Map<String, Value>,
    field: &str,
    default: bool,
    message: &str,
) -> Result<bool, AppError> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(document_validation(message, field, "must be a boolean")),
    }
}

/// Reads the display limits of an extraction request.
///
/// `maxCharacters` is checked first (absent or null means the default), then `includePages`.
///
/// # Errors
///
/// A validation error on `maxCharacters` or `includePages` with the reason of the failing rule.
pub fn extraction_options(request: &Map<String, Value>) -> Result<ExtractionOptions, AppError> {
    let max_characters = match request.get("maxCharacters") {
        None | Some(Value::Null) => DEFAULT_MAX_CHARACTERS,
        Some(value) => {
            let limit = i128::try_from(MAX_CHARACTERS_LIMIT).unwrap_or(i128::MAX);
            let number = match value {
                Value::Number(number) => number
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| number.as_u64().map(i128::from)),
                _ => None,
            };
            let Some(number) = number else {
                return Err(document_validation(
                    EXTRACTION_VALIDATION_MESSAGE,
                    "maxCharacters",
                    "must be an integer",
                ));
            };
            if number <= 0 {
                return Err(document_validation(
                    EXTRACTION_VALIDATION_MESSAGE,
                    "maxCharacters",
                    "must be greater than zero",
                ));
            }
            if number > limit {
                return Err(document_validation(
                    EXTRACTION_VALIDATION_MESSAGE,
                    "maxCharacters",
                    &format!("must be less than or equal to {MAX_CHARACTERS_LIMIT}"),
                ));
            }
            usize::try_from(number).unwrap_or(MAX_CHARACTERS_LIMIT)
        }
    };
    let include_pages =
        optional_boolean(request, "includePages", true, EXTRACTION_VALIDATION_MESSAGE)?;
    Ok(ExtractionOptions {
        max_characters,
        include_pages,
    })
}

/// Reads a document identifier from a text value. Anything that is not a canonical UUID string,
/// including null and non-string values, is `must be a valid UUID`.
///
/// # Errors
///
/// A storage validation error on the `documentId` field.
pub fn document_id_text(text: &str) -> Result<Uuid, AppError> {
    admit_uuid(text).map_err(|_| {
        document_validation(
            STORAGE_VALIDATION_MESSAGE,
            "documentId",
            UuidRefusal::Malformed.reason(),
        )
    })
}

/// Reads the `documentId` member of a request.
///
/// # Errors
///
/// A storage validation error on the `documentId` field when the member is not a UUID string.
pub fn document_id_value(value: Option<&Value>) -> Result<Uuid, AppError> {
    match value {
        Some(Value::String(text)) => document_id_text(text),
        _ => Err(document_validation(
            STORAGE_VALIDATION_MESSAGE,
            "documentId",
            UuidRefusal::Malformed.reason(),
        )),
    }
}

/// Reads the `path` member of a request as text (first check).
///
/// # Errors
///
/// A validation error on `path` with `must not be blank` when the member is absent or blank.
pub fn path_text<'a>(value: Option<&'a Value>, message: &str) -> Result<&'a str, AppError> {
    match value {
        Some(Value::String(text)) if !jobniac_text::str_rules::strip_text(text).is_empty() => {
            Ok(text)
        }
        _ => Err(document_validation(message, "path", "must not be blank")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn basename_accepts_both_separators_and_falls_back() {
        assert_eq!(safe_basename(r"C:\resumes\cv.pdf"), "cv.pdf");
        assert_eq!(safe_basename("a/b/c.pdf"), "c.pdf");
        assert_eq!(safe_basename("dir/"), "stored.pdf");
        assert_eq!(safe_basename(""), "stored.pdf");
    }

    #[test]
    fn storage_checks_empty_before_size_and_header() {
        assert_eq!(
            validate_pdf_content(b"", true).err().map(|e| e.to_string()),
            Some(STORAGE_VALIDATION_MESSAGE.to_owned())
        );
        assert!(validate_pdf_content(b"%PDF-1.4\n", true).is_ok());
        assert!(validate_pdf_content(b"", false).is_err());
    }

    #[test]
    fn options_follow_the_validation_order_and_bounds() {
        let object = json!({"maxCharacters": 0, "includePages": "x"});
        let map = object.as_object().map_or_else(Map::new, Clone::clone);
        let error = extraction_options(&map).err();
        assert!(
            error.is_some(),
            "a zero budget must fail before includePages"
        );
        let defaults = json!({});
        let map = defaults.as_object().map_or_else(Map::new, Clone::clone);
        let options = extraction_options(&map).ok();
        assert_eq!(
            options,
            Some(ExtractionOptions {
                max_characters: DEFAULT_MAX_CHARACTERS,
                include_pages: true,
            })
        );
    }

    #[test]
    fn document_id_refusals_share_one_reason() {
        assert!(document_id_value(None).is_err());
        assert!(document_id_value(Some(&json!(7))).is_err());
        assert!(document_id_text("not-a-uuid").is_err());
    }
}
