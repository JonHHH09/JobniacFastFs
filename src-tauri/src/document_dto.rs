//! IPC mirror of the stored-document results.
//!
//! Identifiers cross as canonical UUID text. The byte size is bounded by the import limit (10 MiB),
//! so it crosses as a number, as the other bounded counts do. The file name is the basename only;
//! the core already reduced it, and the mapping keeps only the final component again as a guard.
//! Extracted text crosses as plain text; React never renders it as markup.

use jobniac_core::clock::format_instant;
use jobniac_core::documents::{
    DocumentListPage, DocumentMetadata, DocumentSummary, PdfText, StoredExtraction, safe_basename,
};
use serde::Serialize;
use specta::Type;

use crate::match_dto::OutOfRange;

/// Whether the documents screen is usable: without a database the intro is
/// replaced and every action is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCapabilitiesDto {
    /// `true` when a local database is configured: the import, the metadata lookup and the page
    /// viewer all need it.
    pub documents: bool,
}

/// The metadata of one stored document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StoredDocumentDto {
    /// The document identifier.
    pub id: String,
    /// The base name of the original file.
    pub display_name: String,
    /// Always `application/pdf`.
    pub media_type: String,
    /// The byte size of the stored content.
    pub byte_size: u32,
}

/// One stored document of the picker listing. The path of the
/// source file is never stored, and no hash or content crosses. The byte size is an `i64` source
/// value, so it crosses as decimal text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StoredDocumentListItemDto {
    /// The document identifier.
    pub id: String,
    /// The base name of the original file.
    pub display_name: String,
    /// The media type, always `application/pdf` for stored PDFs.
    pub media_type: String,
    /// The byte size of the stored content, as decimal text.
    pub byte_size: String,
    /// The page count of the stored extraction, or null when the text was never extracted.
    pub page_count: Option<u32>,
    /// The creation instant as wire text (UTC).
    pub created_at: String,
}

/// One page of the stored-document listing, newest first. A null `nextCursor` means no next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StoredDocumentPageDto {
    /// The documents of this page.
    pub documents: Vec<StoredDocumentListItemDto>,
    /// The signed cursor of the next page.
    pub next_cursor: Option<String>,
}

impl From<&DocumentSummary> for StoredDocumentListItemDto {
    fn from(summary: &DocumentSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            // The core already reduced the name; keeping the final component again is a guard.
            display_name: safe_basename(&summary.original_file_name),
            media_type: summary.media_type.clone(),
            byte_size: summary.byte_size.to_string(),
            page_count: summary.page_count,
            created_at: format_instant(summary.created_at),
        }
    }
}

impl From<&DocumentListPage> for StoredDocumentPageDto {
    fn from(page: &DocumentListPage) -> Self {
        Self {
            documents: page
                .documents
                .iter()
                .map(StoredDocumentListItemDto::from)
                .collect(),
            next_cursor: page.next_cursor.clone(),
        }
    }
}

/// Plain extracted text with its truncation flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TextPreviewDto {
    /// The text, bounded by the display budget.
    pub text: String,
    /// Whether the text was cut at the budget.
    pub truncated: bool,
}

/// A stored document with the preview of its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StoredPreviewDto {
    /// The document the preview is about.
    pub document: StoredDocumentDto,
    /// The text preview.
    pub extraction: TextPreviewDto,
}

/// The PDF chosen in the native dialog. The path itself never crosses
/// IPC: React gets the opaque token that the import and the preview send back, and the base name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SelectedPdfDto {
    /// The unguessable token of the held selection.
    pub selection_token: String,
    /// The base name of the chosen file.
    pub display_name: String,
}

/// Maps the core metadata to the DTO.
///
/// # Errors
///
/// [`OutOfRange`] when the stored byte size is not a valid size of the import limit.
pub fn stored_document(metadata: &DocumentMetadata) -> Result<StoredDocumentDto, OutOfRange> {
    // The presenter's rule: only PDF content, and a non-empty file within the 10 MiB import limit.
    if metadata.media_type != "application/pdf" {
        return Err(OutOfRange);
    }
    let byte_size = u32::try_from(metadata.byte_size).map_err(|_| OutOfRange)?;
    if byte_size == 0 || u64::from(byte_size) > MAX_DOCUMENT_BYTES {
        return Err(OutOfRange);
    }
    Ok(StoredDocumentDto {
        id: metadata.id.to_string(),
        display_name: safe_basename(&metadata.original_file_name),
        media_type: metadata.media_type.clone(),
        byte_size,
    })
}

/// The import limit of a stored PDF in bytes.
pub const MAX_DOCUMENT_BYTES: u64 = 10 * 1024 * 1024;

/// Maps the core text view to the preview DTO.
#[must_use]
pub fn text_preview(text: &PdfText) -> TextPreviewDto {
    TextPreviewDto {
        text: text.text.clone(),
        truncated: text.truncated,
    }
}

/// Maps the core stored extraction to the preview envelope.
///
/// # Errors
///
/// [`OutOfRange`] when the document metadata is out of range.
pub fn stored_preview(stored: &StoredExtraction) -> Result<StoredPreviewDto, OutOfRange> {
    Ok(StoredPreviewDto {
        document: stored_document(&stored.document)?,
        extraction: text_preview(&stored.extraction),
    })
}

#[cfg(test)]
mod tests {
    use jobniac_core::documents::{DocumentListPage, DocumentSummary, safe_basename};
    use time::macros::datetime;
    use uuid::Uuid;

    use super::StoredDocumentPageDto;

    #[test]
    fn the_list_item_carries_text_sizes_and_never_a_path() {
        let page = DocumentListPage {
            documents: vec![DocumentSummary {
                id: Uuid::from_u128(7),
                original_file_name: "a/b/cv.pdf".to_owned(),
                media_type: "application/pdf".to_owned(),
                byte_size: 10_485_760,
                page_count: None,
                created_at: datetime!(2026-10-17 12:00:00.5 UTC),
            }],
            next_cursor: Some("cursor".to_owned()),
        };
        let dto = StoredDocumentPageDto::from(&page);
        assert_eq!(dto.documents[0].display_name, "cv.pdf");
        assert_eq!(dto.documents[0].byte_size, "10485760");
        assert_eq!(dto.documents[0].page_count, None);
        assert_eq!(dto.documents[0].created_at, "2026-10-17T12:00:00.500Z");
        assert_eq!(dto.next_cursor.as_deref(), Some("cursor"));
        let wire = serde_json::to_string(&dto).unwrap_or_default();
        assert!(wire.contains("\"byteSize\":\"10485760\""), "{wire}");
        assert!(!wire.contains("a/b"), "{wire}");
    }

    #[test]
    fn display_name_is_the_core_basename_without_trimming() {
        // One rule for the stored name and the display name: the core's.
        assert_eq!(safe_basename("C:\\Users\\a\\cv.pdf"), "cv.pdf");
        assert_eq!(safe_basename("dir/sub/cv.pdf"), "cv.pdf");
        assert_eq!(safe_basename("dir/"), "stored.pdf");
        assert_eq!(safe_basename("  "), "  ");
    }
}
