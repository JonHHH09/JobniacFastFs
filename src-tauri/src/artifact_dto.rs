//! Artifact DTOs.
//!
//! No path, token or byte array is in any of these types. The page is referenced by its `jobniac-page`
//! URL through [`PageImageRefDto`], the stored document by its canonical ID, and the exported file by
//! its base name only. The `i64` byte size crosses as a decimal string.

use jobniac_core::artifacts::model::windows_basename;
use jobniac_core::documents::DocumentMetadata;
use serde::Serialize;
use specta::Type;

use crate::page_image::PageImageRefDto;

/// Which artifact services are composed. Both are true or both
/// are false: the runtime composes the viewer and the exporter together under the exact flag `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCapabilitiesDto {
    /// The page viewer and the artifact read are admitted.
    pub artifacts: bool,
    /// The native export is admitted; the export controls are rendered only then.
    pub export: bool,
}

/// The summary line of a loaded PDF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSummaryDto {
    /// The base name of the original file. Stored names may carry directory parts; only the last
    /// component is shown.
    pub display_name: String,
    /// The page count of the document.
    pub page_count: u32,
    /// The byte size of the stored PDF, as a decimal string (`i64`).
    pub byte_size: String,
}

/// The state of a loaded PDF after a preview: page 0 is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPreviewStateDto {
    /// The canonical stored document ID.
    pub document_id: String,
    /// The page count of the document.
    pub page_count: u32,
    /// The zero-based page on screen; always 0 after a preview.
    pub current_page: u32,
    /// The summary line of the document.
    pub summary: ArtifactSummaryDto,
    /// The page image of `current_page`.
    pub page: PageImageRefDto,
}

/// The result of a completed export. `null` on a cancelled dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactExportDto {
    /// The base name of the file that was written.
    pub exported_name: String,
}

/// The summary of a stored document shown by the viewer.
#[must_use]
pub fn artifact_summary(document: &DocumentMetadata, page_count: u32) -> ArtifactSummaryDto {
    ArtifactSummaryDto {
        display_name: windows_basename(&document.original_file_name).to_owned(),
        page_count,
        byte_size: document.byte_size.to_string(),
    }
}

/// The preview state of a stored document whose page 0 is rendered as `page`.
#[must_use]
pub fn artifact_preview_state(
    document: &DocumentMetadata,
    page_count: u32,
    page: PageImageRefDto,
) -> ArtifactPreviewStateDto {
    ArtifactPreviewStateDto {
        document_id: document.id.to_string(),
        page_count,
        current_page: 0,
        summary: artifact_summary(document, page_count),
        page,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn metadata(name: &str) -> DocumentMetadata {
        DocumentMetadata {
            id: Uuid::nil(),
            original_file_name: name.to_owned(),
            media_type: "application/pdf".to_owned(),
            byte_size: 4096,
            sha256: "0".repeat(64),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    fn page() -> PageImageRefDto {
        PageImageRefDto {
            document_id: Uuid::nil().to_string(),
            page_index: 0,
            page_count: 2,
            width: 10,
            height: 20,
            revision: "1".to_owned(),
            url: "jobniac-page://localhost/handle/1".to_owned(),
        }
    }

    #[test]
    fn the_summary_shows_only_the_base_name_and_the_size_as_text() {
        let summary = artifact_summary(&metadata("folder\\sub/Example CV.pdf"), 2);
        assert_eq!(summary.display_name, "Example CV.pdf");
        assert_eq!(summary.byte_size, "4096");
        assert_eq!(summary.page_count, 2);
    }

    #[test]
    fn the_preview_state_is_on_page_zero_and_carries_no_path() -> Result<(), serde_json::Error> {
        let state = artifact_preview_state(&metadata("cv.pdf"), 2, page());
        assert_eq!(state.current_page, 0);
        let json = serde_json::to_string(&state)?;
        assert!(json.contains("\"currentPage\":0"));
        assert!(json.contains("\"byteSize\":\"4096\""));
        assert!(!json.contains('\\') && !json.contains("/cv.pdf"));
        Ok(())
    }

    #[test]
    fn the_export_result_serializes_the_exported_name_only() -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(&ArtifactExportDto {
            exported_name: "copy.pdf".to_owned(),
        })?;
        assert_eq!(json, r#"{"exportedName":"copy.pdf"}"#);
        Ok(())
    }
}
