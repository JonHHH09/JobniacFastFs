//! Standalone PDF generation with a logical file name.
//!
//! The file name is only a logical leaf: sanitized to ASCII with the Windows device and dot rules,
//! and never used to select a filesystem destination. The PDF is a classified generated asset with
//! a `STANDALONE_PDF` intent that completes at materialization and is linked to no profile.

use std::sync::Arc;

use jobniac_pdf::layout::{DEFAULT_TITLE, PageNumberLocale, build_pdf_layout};
use jobniac_pdf::render::render_pdf;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::{ApplicationError, validation_error};
use crate::generated_assets::service::sha256_hex;
use crate::generated_assets::{
    GeneratedAssetService, GeneratedReservation, GenerationDescriptor, GenerationPurpose,
    GenerationState, IntentReservation, MaterializationOutcome, validate_generated_pdf,
};
use crate::generation::content::{budget_error, validate_pdf_body};
use crate::generation::generic::{
    GenerationFailure, Publication, compensate, pdf_error, reserve_error, unavailable,
};
use jobniac_text::java::{
    BudgetField, MAX_PREPARATION_UNITS, PreparationBudget, java_is_blank, java_strip,
};

/// Largest raw file name, in code points, before any sanitization.
pub const MAX_RAW_FILE_NAME: usize = 65_536;
/// Largest logical file name after sanitization, in bytes.
pub const MAX_LOGICAL_FILE_NAME: usize = 4096;
/// The name used when the raw name is absent, blank or has no leaf.
const DEFAULT_FILE_NAME: &str = "generated.pdf";

/// The `PDF preparation limit exceeded` refusal for `field`.
fn budget_failure(field: &str) -> ApplicationError {
    validation_error(
        "PDF preparation limit exceeded",
        Some(field),
        Some("exceeds preparation safety budget"),
    )
}

/// Whether `stem` is a Windows reserved device name, case-insensitively (`CON`, `PRN`, `AUX`,
/// `NUL`, `COM1` to `COM9`, `LPT1` to `LPT9`).
fn is_windows_device(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            bytes.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && (b'1'..=b'9').contains(&bytes[3])
        }
    }
}

/// Derives the logical file name of a standalone PDF.
///
/// The raw name is capped, refused when it holds a NUL, and reduced to its leaf after removing
/// trailing separators. Every character outside `A-Z a-z 0-9 . _ -` becomes `_`. A Windows device
/// stem gets a `_` suffix, `.` and `..` become `generated.pdf`, and a `.pdf` suffix is added when
/// missing.
///
/// # Errors
///
/// Fails with the `PDF preparation limit exceeded` refusal over the raw or logical limit, and with
/// `Invalid PDF generation request` for a NUL.
pub fn standalone_file_name(raw: Option<&str>) -> Result<String, ApplicationError> {
    if let Some(raw) = raw {
        if raw.chars().count() > MAX_RAW_FILE_NAME {
            return Err(budget_failure("fileName"));
        }
        if raw.contains('\0') {
            return Err(validation_error(
                "Invalid PDF generation request",
                Some("fileName"),
                Some("must be a valid file name"),
            ));
        }
    }
    let mut base = DEFAULT_FILE_NAME;
    if let Some(raw) = raw.filter(|raw| !java_is_blank(raw)) {
        let trimmed = java_strip(raw).trim_end_matches(['/', '\\']);
        if !trimmed.is_empty() {
            let start = trimmed.rfind(['/', '\\']).map_or(0, |index| index + 1);
            base = &trimmed[start..];
        }
    }
    let mut name: String = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let stem_length = name.split('.').next().map_or(0, str::len);
    if is_windows_device(&name[..stem_length]) {
        name = format!("{}_{}", &name[..stem_length], &name[stem_length..]);
    }
    if name == "." || name == ".." {
        name = DEFAULT_FILE_NAME.to_owned();
    }
    if !name.to_ascii_lowercase().ends_with(".pdf") {
        name.push_str(".pdf");
    }
    // Sanitization is ASCII, so characters and encoded bytes have the same bound.
    if name.len() > MAX_LOGICAL_FILE_NAME {
        return Err(budget_failure("fileName"));
    }
    Ok(name)
}

/// A generated standalone PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedStandalonePdf {
    /// The logical file name.
    pub file_name: String,
    /// The relative path of the committed asset.
    pub path: String,
    /// The byte size of the PDF.
    pub byte_size: usize,
    /// The page count of the PDF.
    pub page_count: usize,
    /// The generation instant in UTC.
    pub generated_at: OffsetDateTime,
    /// The committed reservation.
    pub reservation: GeneratedReservation,
}

/// Standalone PDF generation over the classified asset service and the clock.
pub struct StandaloneGenerationService {
    assets: Arc<GeneratedAssetService>,
    root_id: Uuid,
    clock: Arc<dyn Clock>,
    enabled: bool,
}

impl std::fmt::Debug for StandaloneGenerationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StandaloneGenerationService")
            .field("root_id", &self.root_id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl StandaloneGenerationService {
    /// Builds the service. `enabled` must be the exact opt-in value resolved by the runtime.
    #[must_use]
    pub fn new(
        assets: Arc<GeneratedAssetService>,
        root_id: Uuid,
        clock: Arc<dyn Clock>,
        enabled: bool,
    ) -> Self {
        Self {
            assets,
            root_id,
            clock,
            enabled,
        }
    }

    /// Generates a standalone PDF from an optional title and body.
    ///
    /// Admission runs in order: the body, the body gate, the file name, the title
    /// budget and the layout, then the enabled check, rendering and publication.
    ///
    /// # Errors
    ///
    /// Fails with the preparation, rendering or publication error, with the compensation status of
    /// the claim when one was dispatched.
    pub async fn generate_pdf_file(
        &self,
        file_name: Option<&str>,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<GeneratedStandalonePdf, GenerationFailure> {
        let mut known = Publication::default();
        match self.run(file_name, title, body, &mut known).await {
            Ok(generated) => Ok(generated),
            Err(error) => Err(GenerationFailure {
                error,
                compensation_confirmed: compensate(&self.assets, &known).await,
            }),
        }
    }

    /// The generation steps; any error is returned for compensation.
    async fn run(
        &self,
        file_name: Option<&str>,
        title: Option<&str>,
        body: Option<&str>,
        known: &mut Publication,
    ) -> Result<GeneratedStandalonePdf, ApplicationError> {
        if body.is_some_and(|text| text.chars().count() > MAX_PREPARATION_UNITS) {
            return Err(budget_failure("body"));
        }
        let validated_body = validate_pdf_body(body)?;
        let name = standalone_file_name(file_name)?;
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_text(title).map_err(budget_error)?;
        let resolved_title = match title {
            Some(raw) if !java_is_blank(raw) => java_strip(raw).to_owned(),
            _ => DEFAULT_TITLE.to_owned(),
        };
        let layout = build_pdf_layout(
            Some(resolved_title.as_str()),
            Some(validated_body.as_str()),
            false,
            PageNumberLocale::English,
        )
        .map_err(pdf_error)?;
        if !self.enabled {
            return Err(unavailable("Standalone generation is not enabled"));
        }
        let rendered = render_pdf(
            Some(resolved_title.as_str()),
            Some(validated_body.as_str()),
            false,
            PageNumberLocale::English,
        )
        .map_err(pdf_error)?;
        let data = rendered.data;
        validate_generated_pdf(&data)
            .map_err(|_| unavailable("PDF file could not be generated"))?;
        if rendered.page_count != layout.pages.len() {
            return Err(unavailable("PDF file could not be generated"));
        }
        let size = data.len();
        let descriptor = GenerationDescriptor::new(
            Uuid::new_v4(),
            GenerationPurpose::StandalonePdf,
            None,
            None,
            None,
            None,
        )
        .map_err(|_| unavailable("PDF file could not be generated"))?;
        known.unproven = true;
        let dispatched = self
            .assets
            .reserve_classified_pdf(&data, &name, &descriptor)
            .await
            .map_err(reserve_error)?;
        if dispatched.intent.descriptor != descriptor
            || dispatched.intent.state != GenerationState::Pending
            || dispatched.reservation.asset.root_id() != self.root_id
            || dispatched.reservation.asset.sha256() != sha256_hex(&data)
            || dispatched.reservation.asset.size_bytes() != size as u64
        {
            return Err(unavailable("Standalone reservation is unproven"));
        }
        known.claim = Some(dispatched.clone());
        known.unproven = false;
        let active = self
            .assets
            .materialize_classified(&dispatched)
            .await
            .map_err(|error| unavailable(error.0))?;
        let completed: IntentReservation = active.claim;
        if active.outcome != MaterializationOutcome::Active
            || completed.reservation != dispatched.reservation
            || completed.intent.descriptor != dispatched.intent.descriptor
            || completed.intent.created_at != dispatched.intent.created_at
            || completed.intent.state != GenerationState::Completed
        {
            return Err(unavailable("Standalone publication is unproven"));
        }
        let now = self.clock.now().to_offset(time::UtcOffset::UTC);
        match completed.intent.finished_at {
            Some(finished_at) if now >= finished_at => {}
            _ => return Err(unavailable("Invalid standalone generation clock")),
        }
        Ok(GeneratedStandalonePdf {
            path: completed.reservation.asset.relative_path().to_owned(),
            file_name: name,
            byte_size: size,
            page_count: layout.pages.len(),
            generated_at: now,
            reservation: completed.reservation,
        })
    }
}
