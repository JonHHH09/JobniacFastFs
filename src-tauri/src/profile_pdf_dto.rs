//! IPC mirror of the stored-PDF profile ingestion.
//!
//! The core returns a result dictionary. This module maps it to typed values: the status
//! must be one of the five ingestion statuses, the matched-on reasons are mapped to the enum in the
//! order of the English labels, and any other value is refused as outside its range (the whole
//! response fails; no status is guessed). Identifiers cross as canonical UUID text; the revision of
//! a replacement crosses as a string.

use jobniac_core::ids::is_canonical_uuid_text;
use serde::Serialize;
use serde_json::Value;
use specta::Type;

use crate::match_dto::OutOfRange;

/// Whether the profile-from-PDF screen is usable without a database
/// the intro is replaced and every action is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePdfCapabilitiesDto {
    /// `true` when a local database is configured: the ingestion, the confirmation and the source
    /// lookup all need it.
    pub profile_pdf: bool,
}

/// The reason a candidate profile matched, in the order of the English labels (`Email`, `GitHub`,
/// `LinkedIn`, `Other profile link`, `Profile ID`). Declaration order is the sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Type)]
pub enum MatchedOn {
    /// The profile has the same email.
    #[serde(rename = "email")]
    Email,
    /// The profile has the same GitHub link.
    #[serde(rename = "github")]
    GitHub,
    /// The profile has the same LinkedIn link.
    #[serde(rename = "linkedin")]
    LinkedIn,
    /// The profile has the same link of another type.
    #[serde(rename = "otherLink")]
    OtherLink,
    /// The profile has the same identifier.
    #[serde(rename = "profileId")]
    ProfileId,
}

impl MatchedOn {
    /// Maps a core reason (`email`, `link:<type>`, `profileId`) to the enum.
    ///
    /// # Errors
    ///
    /// [`OutOfRange`] for any other reason.
    pub fn from_reason(reason: &str) -> Result<Self, OutOfRange> {
        match reason {
            "email" => Ok(Self::Email),
            "profileId" => Ok(Self::ProfileId),
            "link:linkedin" => Ok(Self::LinkedIn),
            "link:github" => Ok(Self::GitHub),
            other
                if other
                    .strip_prefix("link:")
                    .is_some_and(|kind| !kind.is_empty()) =>
            {
                Ok(Self::OtherLink)
            }
            _ => Err(OutOfRange),
        }
    }
}

/// The five ingestion statuses. The UI copy is chosen by this enum; any other core status is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum IngestStatus {
    /// A new profile was written with its source link.
    #[serde(rename = "createdProfile")]
    CreatedProfile,
    /// An existing profile was replaced with its source link.
    #[serde(rename = "updatedProfile")]
    UpdatedProfile,
    /// The stored PDF was already linked; nothing was written.
    #[serde(rename = "reusedExistingSource")]
    ReusedExistingSource,
    /// A single possible existing profile was found; nothing was written.
    #[serde(rename = "duplicateProfileCandidate")]
    DuplicateProfileCandidate,
    /// Several possible existing profiles were found; nothing was written.
    #[serde(rename = "ambiguousProfileCandidates")]
    AmbiguousProfileCandidates,
}

impl IngestStatus {
    /// Maps the status string.
    ///
    /// # Errors
    ///
    /// [`OutOfRange`] for any other status.
    pub fn from_wire(text: &str) -> Result<Self, OutOfRange> {
        match text {
            "CREATED_PROFILE" => Ok(Self::CreatedProfile),
            "UPDATED_PROFILE" => Ok(Self::UpdatedProfile),
            "REUSED_EXISTING_SOURCE" => Ok(Self::ReusedExistingSource),
            "DUPLICATE_PROFILE_CANDIDATE" => Ok(Self::DuplicateProfileCandidate),
            "AMBIGUOUS_PROFILE_CANDIDATES" => Ok(Self::AmbiguousProfileCandidates),
            _ => Err(OutOfRange),
        }
    }

    /// Whether the status describes a candidate that was not written.
    #[must_use]
    pub const fn is_candidate(self) -> bool {
        matches!(
            self,
            Self::DuplicateProfileCandidate | Self::AmbiguousProfileCandidates
        )
    }
}

/// A candidate profile that was not written (`duplicateProfileCandidate` or
/// `ambiguousProfileCandidates`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CandidateDto {
    /// The candidate profile, when one was named.
    pub profile_id: Option<String>,
    /// The reasons the candidate matched, sorted in label order.
    pub matched_on: Vec<MatchedOn>,
    /// Whether the stored PDF is already linked to a profile.
    pub already_linked: bool,
}

/// The result of an ingestion: the status, the document and the rows written, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IngestResultDto {
    /// The ingestion status.
    pub status: IngestStatus,
    /// The stored document the result is about.
    pub document_id: String,
    /// The profile written or reused, when there is one.
    pub profile_id: Option<String>,
    /// The PDF extraction of the document.
    pub extraction_id: Option<String>,
    /// The source link written or reused, when there is one.
    pub source_link_id: Option<String>,
    /// The candidate, for the two candidate statuses only.
    pub candidate: Option<CandidateDto>,
}

/// One step of the ingestion: the result, or the replacement that must be confirmed first. A
/// confirmation step never reaches the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum IngestStepDto {
    /// The ingestion ran; the result is final.
    Result {
        /// The result of the ingestion.
        result: IngestResultDto,
    },
    /// A replacement was staged; React shows the confirmation dialog and sends the token back.
    #[serde(rename_all = "camelCase")]
    ConfirmationRequired {
        /// The opaque token of the staged replacement, held in Rust.
        token: String,
        /// The profile to replace.
        existing_profile_id: String,
        /// The revision the replacement is checked against (a string).
        expected_revision: String,
        /// The stored document the replacement is built from.
        document_id: String,
    },
}

/// The source link of a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceViewDto {
    /// The source link identifier.
    pub source_link_id: String,
    /// The profile the source belongs to.
    pub profile_id: String,
    /// The PDF extraction the source links to.
    pub extraction_id: String,
    /// The source type; only the resume PDF exists.
    pub source_type: SourceType,
    /// The creation instant as wire text.
    pub created_at: String,
}

/// The source types of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum SourceType {
    /// A resume PDF (`resume_pdf` in the database).
    #[serde(rename = "resumePdf")]
    ResumePdf,
}

/// The state the window returns to after a cancelled replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum RestoredState {
    /// A previous result is still shown.
    #[serde(rename = "ready")]
    Ready,
    /// Nothing is shown yet.
    #[serde(rename = "idle")]
    Idle,
}

/// The response of a cancelled replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CancelReplacementDto {
    /// The state the screen returns to.
    pub restored_state: RestoredState,
}

/// Reads a required string member of a core value.
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, OutOfRange> {
    value.get(key).and_then(Value::as_str).ok_or(OutOfRange)
}

/// Reads an optional string member; absent or null is `None`, any other type is out of range.
fn optional_text<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, OutOfRange> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(OutOfRange),
    }
}

/// Reads the matched-on reasons: an array of strings, mapped, sorted and without duplicates. An
/// unknown reason is dropped; only a non-array or a non-text item
/// is refused.
fn matched_on(value: &Value) -> Result<Vec<MatchedOn>, OutOfRange> {
    let reasons = match value.get("matchedOn") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(OutOfRange),
    };
    let mut mapped = Vec::with_capacity(reasons.len());
    for reason in reasons {
        let reason = reason.as_str().ok_or(OutOfRange)?;
        if let Ok(known) = MatchedOn::from_reason(reason) {
            mapped.push(known);
        }
    }
    mapped.sort_unstable();
    mapped.dedup();
    Ok(mapped)
}

/// Reads a required member that must be a canonical UUID text (the presenter's identifier rule).
fn uuid_text<'a>(value: &'a Value, key: &str) -> Result<&'a str, OutOfRange> {
    let text = text(value, key)?;
    if is_canonical_uuid_text(text) {
        Ok(text)
    } else {
        Err(OutOfRange)
    }
}

/// Reads an optional member; when present (not null) it must be a canonical UUID text.
fn optional_uuid_text<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, OutOfRange> {
    match optional_text(value, key)? {
        Some(text) if !is_canonical_uuid_text(text) => Err(OutOfRange),
        other => Ok(other),
    }
}

/// Maps the core ingestion result to the typed DTO. The already-linked flag follows the presenter:
/// a duplicate candidate whose only match is the profile ID (the core does not set the link flag).
///
/// # Errors
///
/// [`OutOfRange`] when the status is outside its range, or an identifier is not a UUID text where
/// the presenter requires one.
pub fn ingest_result(value: &Value) -> Result<IngestResultDto, OutOfRange> {
    let status = IngestStatus::from_wire(text(value, "status")?)?;
    let document_id = uuid_text(value, "documentId")?.to_owned();
    let extraction_id = uuid_text(value, "pdfExtractionId")?.to_owned();
    let candidate = if status.is_candidate() {
        let matched_on = matched_on(value)?;
        let already_linked = status == IngestStatus::DuplicateProfileCandidate
            && matched_on == [MatchedOn::ProfileId];
        Some(CandidateDto {
            profile_id: optional_uuid_text(value, "candidateProfileId")?.map(str::to_owned),
            matched_on,
            already_linked,
        })
    } else {
        None
    };
    let (profile_id, source_link_id) = if status.is_candidate() {
        (
            optional_uuid_text(value, "profileId")?.map(str::to_owned),
            optional_uuid_text(value, "sourceLinkId")?.map(str::to_owned),
        )
    } else {
        (
            Some(uuid_text(value, "profileId")?.to_owned()),
            Some(uuid_text(value, "sourceLinkId")?.to_owned()),
        )
    };
    Ok(IngestResultDto {
        status,
        document_id,
        profile_id,
        extraction_id: Some(extraction_id),
        source_link_id,
        candidate,
    })
}

/// Maps the core source link to the typed DTO. Only `resume_pdf` is a source type.
///
/// # Errors
///
/// [`OutOfRange`] for any other source type or a missing member.
pub fn source_view(value: &Value) -> Result<SourceViewDto, OutOfRange> {
    let source_type = match text(value, "sourceType")? {
        "resume_pdf" => SourceType::ResumePdf,
        _ => return Err(OutOfRange),
    };
    Ok(SourceViewDto {
        source_link_id: text(value, "id")?.to_owned(),
        profile_id: text(value, "profileId")?.to_owned(),
        extraction_id: text(value, "pdfExtractionId")?.to_owned(),
        source_type,
        created_at: aware_time(text(value, "createdAt")?)?,
    })
}

/// Parses a timezone-aware RFC 3339 time and returns its normalized form (the presenter's rule: a
/// naive time is refused, and the shown value is the normalized one).
fn aware_time(text: &str) -> Result<String, OutOfRange> {
    let parsed = time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map_err(|_| OutOfRange)?;
    parsed
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| OutOfRange)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reasons_map_to_the_five_labels_and_sort_in_label_order() -> Result<(), OutOfRange> {
        let value = json!({
            "status": "DUPLICATE_PROFILE_CANDIDATE",
            "documentId": "00000000-0000-4000-8000-000000000001",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
            "candidateProfileId": "00000000-0000-4000-8000-000000000003",
            "matchedOn": ["link:website", "profileId", "link:linkedin", "email", "link:github", "email"],
            "existingProfileLink": false,
        });
        let result = ingest_result(&value)?;
        let candidate = result.candidate.ok_or(OutOfRange)?;
        assert_eq!(
            candidate.matched_on,
            vec![
                MatchedOn::Email,
                MatchedOn::GitHub,
                MatchedOn::LinkedIn,
                MatchedOn::OtherLink,
                MatchedOn::ProfileId,
            ]
        );
        assert!(!candidate.already_linked);
        Ok(())
    }

    #[test]
    fn an_unknown_status_or_reason_is_refused() {
        let unknown_status =
            json!({"status": "WRITTEN", "documentId": "x", "pdfExtractionId": "y"});
        assert_eq!(ingest_result(&unknown_status), Err(OutOfRange));
        assert_eq!(MatchedOn::from_reason("link:"), Err(OutOfRange));
        assert_eq!(MatchedOn::from_reason("name"), Err(OutOfRange));
    }

    #[test]
    fn a_created_profile_has_no_candidate_and_keeps_its_ids() -> Result<(), OutOfRange> {
        let value = json!({
            "status": "CREATED_PROFILE",
            "documentId": "00000000-0000-4000-8000-000000000001",
            "profileId": "00000000-0000-4000-8000-000000000004",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
            "sourceLinkId": "00000000-0000-4000-8000-000000000005",
            "matchedOn": [],
        });
        let result = ingest_result(&value)?;
        assert_eq!(result.status, IngestStatus::CreatedProfile);
        assert!(result.candidate.is_none());
        assert!(result.source_link_id.is_some());
        Ok(())
    }

    #[test]
    fn a_duplicate_matched_only_on_the_profile_id_is_already_linked() -> Result<(), OutOfRange> {
        let value = json!({
            "status": "DUPLICATE_PROFILE_CANDIDATE",
            "documentId": "00000000-0000-4000-8000-000000000001",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
            "candidateProfileId": "00000000-0000-4000-8000-000000000003",
            "matchedOn": ["profileId"],
            "existingProfileLink": false,
        });
        let candidate = ingest_result(&value)?.candidate.ok_or(OutOfRange)?;
        assert!(candidate.already_linked);
        let other = json!({
            "status": "DUPLICATE_PROFILE_CANDIDATE",
            "documentId": "00000000-0000-4000-8000-000000000001",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
            "candidateProfileId": "00000000-0000-4000-8000-000000000003",
            "matchedOn": ["email", "profileId"],
        });
        let candidate = ingest_result(&other)?.candidate.ok_or(OutOfRange)?;
        assert!(!candidate.already_linked);
        Ok(())
    }

    #[test]
    fn an_unknown_reason_is_dropped_and_a_non_uuid_identifier_is_refused() -> Result<(), OutOfRange>
    {
        let value = json!({
            "status": "AMBIGUOUS_PROFILE_CANDIDATES",
            "documentId": "00000000-0000-4000-8000-000000000001",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
            "matchedOn": ["name", "email"],
        });
        let candidate = ingest_result(&value)?.candidate.ok_or(OutOfRange)?;
        assert_eq!(candidate.matched_on, vec![MatchedOn::Email]);
        let bad_document = json!({
            "status": "AMBIGUOUS_PROFILE_CANDIDATES",
            "documentId": "not-a-uuid",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
        });
        assert_eq!(ingest_result(&bad_document), Err(OutOfRange));
        let created_without_link = json!({
            "status": "CREATED_PROFILE",
            "documentId": "00000000-0000-4000-8000-000000000001",
            "profileId": "00000000-0000-4000-8000-000000000004",
            "pdfExtractionId": "00000000-0000-4000-8000-000000000002",
        });
        assert_eq!(ingest_result(&created_without_link), Err(OutOfRange));
        Ok(())
    }

    #[test]
    fn a_source_type_other_than_resume_pdf_is_refused() {
        let value = json!({
            "id": "a", "profileId": "b", "pdfExtractionId": "c",
            "sourceType": "link", "createdAt": "2026-01-01T00:00:00Z",
        });
        assert_eq!(source_view(&value), Err(OutOfRange));
    }

    #[test]
    fn the_confirmation_step_serializes_with_the_kind_tag() -> Result<(), serde_json::Error> {
        let step = IngestStepDto::ConfirmationRequired {
            token: "t".to_owned(),
            existing_profile_id: "p".to_owned(),
            expected_revision: "3".to_owned(),
            document_id: "d".to_owned(),
        };
        let json = serde_json::to_value(step)?;
        assert_eq!(json["kind"], "confirmationRequired");
        assert_eq!(json["expectedRevision"], "3");
        Ok(())
    }
}
