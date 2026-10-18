//! Generated-asset lifecycle: immutable identities, classified generation intents with leases and
//! fencing, and the ports and service that the SQLite and file adapters implement.
//!
//! Pure rules live here; the transactions live in `jobniac_infra::sqlite::generated`.

pub mod identity;
pub mod intents;
pub mod profile_links;
pub mod service;
pub mod standalone;

pub use profile_links::{
    InvalidResumeLink, MAX_REVISION, ProfileResumeDocument, RESUME_TYPES, ReplaceResult,
    admit_profile_resume_link, normalize_resume_type, validate_revision,
};

pub use identity::{
    AssetIdentity, GeneratedReservation, InvalidGeneratedIdentity, MAX_FILE_NAME_BYTES,
    MAX_PDF_BYTES, MaterializationOutcome, PublicationFailure, PublicationFailureCategory,
    PublicationOutcome, validate_file_name, validate_generated_pdf,
};
pub use intents::{
    GENERATION_LEASE, GenerationDescriptor, GenerationIntent, GenerationPurpose, GenerationState,
    IntentReservation, validate_binding_intents,
};
pub use service::{
    CLAIM_LEASE, ClassifiedAssetResult, GeneratedAssetRepository, GeneratedAssetService,
    GeneratedAssetUnavailable, GeneratedAssetWriter, PreparationOutcome, RETRY_BATCH_SIZE,
    ReserveError, sha256_hex,
};
