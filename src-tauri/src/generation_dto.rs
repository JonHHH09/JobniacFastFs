//! IPC shapes of the Generation screen.
//!
//! The requests are typed per kind: a resume request carries only the fields of its own kind, so an
//! irrelevant field cannot be sent. Identifiers cross as canonical lowercase text.
//! A generated PDF is referenced only by its opaque `documentId`; the file path and the
//! generated-file metadata never cross IPC. `byteSize` is a `u32` (at most 10 MiB by the output
//! rule), not an `i64`, so it is a JSON number.

use jobniac_core::documents::model::MAX_FILE_BYTES;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use specta::Type;

/// The largest output size the screen accepts: 10 MiB.
pub const MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;

/// A standalone PDF from free text. Empty text is sent as it is typed; the
/// facade decides what an empty value means.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GenerationStandaloneRequest {
    /// The optional PDF file name (leaf name only; the facade enforces the rules).
    pub file_name: String,
    /// The optional title.
    pub title: String,
    /// The PDF body. The facade enforces the preparation budget.
    pub body: String,
}

/// A resume generation request, one variant per kind. The wire `kind`
/// names the variant; only that variant's fields exist.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(tag = "kind")]
pub enum GenerationResumeRequest {
    /// The master resume of a profile (kind 1).
    #[serde(rename = "masterResume")]
    MasterResume {
        /// The profile identifier.
        #[serde(rename = "profileId")]
        profile_id: String,
    },
    /// The Canadian English resume of a profile (kind 2).
    #[serde(rename = "canadianResume")]
    CanadianResume {
        /// The profile identifier.
        #[serde(rename = "profileId")]
        profile_id: String,
    },
    /// The Canadian French resume of a profile (kind 3).
    #[serde(rename = "canadianFrenchResume")]
    CanadianFrenchResume {
        /// The profile identifier.
        #[serde(rename = "profileId")]
        profile_id: String,
    },
    /// The bilingual Germany resume of a profile for a job (kind 4).
    #[serde(rename = "germanyResume")]
    GermanyResume {
        /// The profile identifier.
        #[serde(rename = "profileId")]
        profile_id: String,
        /// The job identifier.
        #[serde(rename = "jobId")]
        job_id: String,
        /// Whether the projects section is included.
        #[serde(rename = "includeProjects")]
        include_projects: bool,
    },
    /// The German cover letter for a job, against the exact Germany resume selected (kind 5).
    #[serde(rename = "germanCoverLetter")]
    GermanCoverLetter {
        /// The profile identifier.
        #[serde(rename = "profileId")]
        profile_id: String,
        /// The job identifier.
        #[serde(rename = "jobId")]
        job_id: String,
        /// The Germany resume identifier the letter is built against.
        #[serde(rename = "resumeId")]
        resume_id: String,
    },
}

/// The language prefix of an output label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum LabelPrefixDto {
    /// A single-PDF output.
    #[serde(rename = "PDF")]
    Pdf,
    /// The English variant of a Germany resume.
    #[serde(rename = "EN")]
    En,
    /// The German variant of a Germany resume.
    #[serde(rename = "DE")]
    De,
}

/// The shape of a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum GenerationVariantDto {
    /// One PDF.
    #[serde(rename = "single")]
    Single,
    /// The English and German PDFs of a Germany resume.
    #[serde(rename = "germanTailored")]
    GermanTailored,
}

/// One generated PDF of a result. The `documentId` is the only handle React holds for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GenerationOutputDto {
    /// The stored document identifier of the PDF (canonical lowercase).
    pub document_id: String,
    /// The language prefix of the label.
    pub label_prefix: LabelPrefixDto,
    /// The size of the PDF in bytes (1 to 10 MiB).
    pub byte_size: u32,
    /// The page count of the PDF.
    pub page_count: u32,
}

/// The result of one generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GenerationResultDto {
    /// Single PDF or the German bilingual pair.
    pub variant: GenerationVariantDto,
    /// The Germany resume identifier of a German result; `null` otherwise.
    pub resume_id: Option<String>,
    /// The generated PDFs, in order.
    pub outputs: Vec<GenerationOutputDto>,
}

/// Which generation services are composed and usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GenerationCapabilitiesDto {
    /// `true` only when generation is admitted: the exact `JOBNIAC_GENERATED_ENABLED=1` flag and an
    /// admitted root.
    pub generation: bool,
}

/// Builds the facade request of a standalone PDF.
#[must_use]
pub fn standalone_value(request: &GenerationStandaloneRequest) -> Value {
    json!({
        "fileName": request.file_name,
        "title": request.title,
        "body": request.body,
    })
}

/// The output-size and page-count rules of a generated file: the size is 1 to
/// 10 MiB; the page count is positive and within the bound (`MAX_FILE_BYTES`).
fn pdf_size(file: &Value) -> Option<(u32, u32)> {
    let size = file.get("byteSize")?.as_u64()?;
    let pages = file.get("pageCount")?.as_u64()?;
    if !(1..=MAX_OUTPUT_BYTES).contains(&size) || !(1..=MAX_FILE_BYTES as u64).contains(&pages) {
        return None;
    }
    Some((u32::try_from(size).ok()?, u32::try_from(pages).ok()?))
}

/// The bilingual result of a Germany resume. The facade response
/// carries `resumeId` and exactly two variants in the order `en`, `de`; each variant's size and page
/// count are in its `generatedFile`. Only the identifiers and the metadata are mapped, so no path
/// crosses IPC.
///
/// Returns `None` when the response does not have that shape (the caller maps it to the generic
/// failure).
#[must_use]
pub fn german_tailored_result(response: &Value) -> Option<GenerationResultDto> {
    let resume_id = response.get("resumeId")?.as_str()?;
    let resume_id = jobniac_core::ids::admit_uuid(resume_id).ok()?.to_string();
    let variants = response.get("variants")?.as_array()?;
    let [english, german] = variants.as_slice() else {
        return None;
    };
    let outputs = [
        (english, LabelPrefixDto::En, "en"),
        (german, LabelPrefixDto::De, "de"),
    ]
    .into_iter()
    .map(|(variant, label_prefix, language)| {
        if variant.get("language")?.as_str()? != language {
            return None;
        }
        let document_id = variant.get("documentId")?.as_str()?;
        let document_id = jobniac_core::ids::admit_uuid(document_id).ok()?.to_string();
        let (byte_size, page_count) = pdf_size(variant.get("generatedFile")?)?;
        Some(GenerationOutputDto {
            document_id,
            label_prefix,
            byte_size,
            page_count,
        })
    })
    .collect::<Option<Vec<_>>>()?;
    Some(GenerationResultDto {
        variant: GenerationVariantDto::GermanTailored,
        resume_id: Some(resume_id),
        outputs,
    })
}

/// The single-PDF result of a standalone or a profile resume operation. `file` holds the size and
/// page count: the response itself for a standalone PDF, its `generatedFile` for a resume.
///
/// Returns `None` when the response does not have the shape the screen can show (the caller maps
/// it to the generic failure).
#[must_use]
pub fn single_result(response: &Value, file: &Value) -> Option<GenerationResultDto> {
    let document_id = response.get("documentId")?.as_str()?;
    let document_id = jobniac_core::ids::admit_uuid(document_id).ok()?.to_string();
    let (byte_size, page_count) = pdf_size(file)?;
    Some(GenerationResultDto {
        variant: GenerationVariantDto::Single,
        resume_id: None,
        outputs: vec![GenerationOutputDto {
            document_id,
            label_prefix: LabelPrefixDto::Pdf,
            byte_size,
            page_count,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000c1";

    #[test]
    fn a_resume_request_carries_only_the_fields_of_its_kind() -> Result<(), serde_json::Error> {
        let request: GenerationResumeRequest = serde_json::from_value(json!({
            "kind": "masterResume",
            "profileId": DOCUMENT_ID,
        }))?;
        assert_eq!(
            request,
            GenerationResumeRequest::MasterResume {
                profile_id: DOCUMENT_ID.to_owned()
            }
        );
        // A field that the kind does not have is not part of the type; an unknown kind is refused.
        let unknown = serde_json::from_value::<GenerationResumeRequest>(json!({
            "kind": "standalone",
            "profileId": DOCUMENT_ID,
        }));
        assert!(unknown.is_err());
        Ok(())
    }

    #[test]
    fn a_germany_cover_letter_request_names_its_three_identifiers() -> Result<(), serde_json::Error>
    {
        let request: GenerationResumeRequest = serde_json::from_value(json!({
            "kind": "germanCoverLetter",
            "profileId": DOCUMENT_ID,
            "jobId": DOCUMENT_ID,
            "resumeId": DOCUMENT_ID,
        }))?;
        assert!(matches!(
            request,
            GenerationResumeRequest::GermanCoverLetter { .. }
        ));
        Ok(())
    }

    #[test]
    fn a_standalone_request_maps_to_the_facade_keys() {
        let request = GenerationStandaloneRequest {
            file_name: "cv.pdf".to_owned(),
            title: "Ada Example".to_owned(),
            body: "Synthetic body".to_owned(),
        };
        assert_eq!(
            standalone_value(&request),
            json!({"fileName": "cv.pdf", "title": "Ada Example", "body": "Synthetic body"})
        );
    }

    #[test]
    fn a_single_result_keeps_the_document_id_and_drops_every_path() -> Result<(), serde_json::Error>
    {
        let response = json!({
            "profileId": DOCUMENT_ID,
            "documentId": DOCUMENT_ID,
            "filePath": "generated/2026/10/secret-name.pdf",
            "generatedFile": {"fileName": "x.pdf", "path": "generated/x.pdf", "byteSize": 2048, "pageCount": 2},
        });
        let result = single_result(&response, &response["generatedFile"])
            .ok_or_else(|| serde::de::Error::custom("no result"))?;
        let json = serde_json::to_value(&result)?;
        assert_eq!(
            json,
            json!({
                "variant": "single",
                "resumeId": null,
                "outputs": [{
                    "documentId": DOCUMENT_ID,
                    "labelPrefix": "PDF",
                    "byteSize": 2048,
                    "pageCount": 2
                }]
            })
        );
        assert!(!json.to_string().contains("generated/"));
        Ok(())
    }

    #[test]
    fn a_single_result_refuses_an_out_of_range_size_or_page_count() {
        let response = json!({"documentId": DOCUMENT_ID});
        let empty = json!({"byteSize": 0, "pageCount": 1});
        let too_large = json!({"byteSize": MAX_OUTPUT_BYTES + 1, "pageCount": 1});
        let no_pages = json!({"byteSize": 10, "pageCount": 0});
        assert!(single_result(&response, &empty).is_none());
        assert!(single_result(&response, &too_large).is_none());
        assert!(single_result(&response, &no_pages).is_none());
    }

    #[test]
    fn a_single_result_refuses_a_missing_or_malformed_document_id() {
        let file = json!({"byteSize": 10, "pageCount": 1});
        assert!(single_result(&json!({}), &file).is_none());
        assert!(single_result(&json!({"documentId": "not-a-uuid"}), &file).is_none());
    }
}
