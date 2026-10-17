//! Immutable classified generation proofs.
//!
//! A descriptor names the operation and its output scope; an intent is the durable lease of one
//! asset's generation; an [`IntentReservation`] pairs the intent with the reserved asset. Every
//! constructor validates the invariants, so an invalid proof cannot be represented.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::identity::{GeneratedReservation, InvalidGeneratedIdentity, generation_instant};

/// Lease length of a pending generation (five minutes).
pub const GENERATION_LEASE: Duration = Duration::minutes(5);

/// Purpose of a classified generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationPurpose {
    /// A standalone generated PDF with no scope.
    StandalonePdf,
    /// A profile resume; scope is `master_resume`, `canadian_resume` or `canadian_resume_fr`.
    ProfileResume,
    /// A German resume; scope is `en` or `de`.
    GermanResume,
    /// A German cover letter; scope is `de`.
    GermanCoverLetter,
}

impl GenerationPurpose {
    /// The stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StandalonePdf => "STANDALONE_PDF",
            Self::ProfileResume => "PROFILE_RESUME",
            Self::GermanResume => "GERMAN_RESUME",
            Self::GermanCoverLetter => "GERMAN_COVER_LETTER",
        }
    }

    /// Parses the stored value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "STANDALONE_PDF" => Self::StandalonePdf,
            "PROFILE_RESUME" => Self::ProfileResume,
            "GERMAN_RESUME" => Self::GermanResume,
            "GERMAN_COVER_LETTER" => Self::GermanCoverLetter,
            _ => return None,
        })
    }
}

/// State of a generation intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationState {
    /// Leased and not yet finished.
    Pending,
    /// Published and linked.
    Completed,
    /// Fenced and retired without a reference.
    Abandoned,
}

impl GenerationState {
    /// The stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Completed => "COMPLETED",
            Self::Abandoned => "ABANDONED",
        }
    }

    /// Parses the stored value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "PENDING" => Self::Pending,
            "COMPLETED" => Self::Completed,
            "ABANDONED" => Self::Abandoned,
            _ => return None,
        })
    }
}

/// The operation a generated output belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationDescriptor {
    /// Operation identifier shared by the outputs of one operation.
    pub operation_id: Uuid,
    /// Purpose of the output.
    pub purpose: GenerationPurpose,
    /// Owning profile, when the purpose has one.
    pub profile_id: Option<Uuid>,
    /// Job, for German purposes.
    pub job_id: Option<Uuid>,
    /// Resume, for German purposes.
    pub resume_id: Option<Uuid>,
    /// Output scope (at most 32 characters).
    pub output_scope: Option<String>,
}

impl GenerationDescriptor {
    /// Validates the purpose shape.
    ///
    /// # Errors
    ///
    /// Fails for an invalid scope, a missing or extra identifier, or an unknown purpose shape.
    pub fn new(
        operation_id: Uuid,
        purpose: GenerationPurpose,
        profile_id: Option<Uuid>,
        job_id: Option<Uuid>,
        resume_id: Option<Uuid>,
        output_scope: Option<String>,
    ) -> Result<Self, InvalidGeneratedIdentity> {
        let descriptor = Self {
            operation_id,
            purpose,
            profile_id,
            job_id,
            resume_id,
            output_scope,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Re-checks the invariants of a descriptor built by hand.
    ///
    /// # Errors
    ///
    /// Fails with the same refusal as [`GenerationDescriptor::new`].
    pub fn validate(&self) -> Result<(), InvalidGeneratedIdentity> {
        if self
            .output_scope
            .as_ref()
            .is_some_and(|scope| scope.chars().count() > 32)
        {
            return Err(InvalidGeneratedIdentity);
        }
        let scope = self.output_scope.as_deref();
        let valid = match self.purpose {
            GenerationPurpose::StandalonePdf => {
                self.profile_id.is_none()
                    && self.job_id.is_none()
                    && self.resume_id.is_none()
                    && scope.is_none()
            }
            GenerationPurpose::ProfileResume => {
                self.profile_id.is_some()
                    && self.job_id.is_none()
                    && self.resume_id.is_none()
                    && matches!(
                        scope,
                        Some("master_resume" | "canadian_resume" | "canadian_resume_fr")
                    )
            }
            GenerationPurpose::GermanResume => {
                self.profile_id.is_some()
                    && self.job_id.is_some()
                    && self.resume_id.is_some()
                    && matches!(scope, Some("en" | "de"))
            }
            GenerationPurpose::GermanCoverLetter => {
                self.profile_id.is_some()
                    && self.job_id.is_some()
                    && self.resume_id.is_some()
                    && scope == Some("de")
            }
        };
        if valid {
            Ok(())
        } else {
            Err(InvalidGeneratedIdentity)
        }
    }
}

/// A durable intent of one asset's classified generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationIntent {
    /// The asset this intent classifies.
    pub asset_id: Uuid,
    /// The operation descriptor.
    pub descriptor: GenerationDescriptor,
    /// The state.
    pub state: GenerationState,
    /// Current fence token; present only while pending.
    pub lease_token: Option<Uuid>,
    /// Lease start; present only while pending.
    pub lease_started_at: Option<OffsetDateTime>,
    /// Lease expiry; present only while pending.
    pub lease_expires_at: Option<OffsetDateTime>,
    /// Creation instant in UTC.
    pub created_at: OffsetDateTime,
    /// Finish instant in UTC; present only for terminal states.
    pub finished_at: Option<OffsetDateTime>,
}

impl GenerationIntent {
    /// Validates and normalizes an intent.
    ///
    /// # Errors
    ///
    /// Fails when the lease, finish or creation instants violate the state shape.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the persisted intent columns"
    )]
    pub fn new(
        asset_id: Uuid,
        descriptor: GenerationDescriptor,
        state: GenerationState,
        lease_token: Option<Uuid>,
        lease_started_at: Option<OffsetDateTime>,
        lease_expires_at: Option<OffsetDateTime>,
        created_at: OffsetDateTime,
        finished_at: Option<OffsetDateTime>,
    ) -> Result<Self, InvalidGeneratedIdentity> {
        let intent = Self {
            asset_id,
            descriptor,
            state,
            lease_token,
            lease_started_at: lease_started_at.map(generation_instant).transpose()?,
            lease_expires_at: lease_expires_at.map(generation_instant).transpose()?,
            created_at: generation_instant(created_at)?,
            finished_at: finished_at.map(generation_instant).transpose()?,
        };
        intent.validate()?;
        Ok(intent)
    }

    /// A new pending intent leased from `now` for [`GENERATION_LEASE`].
    ///
    /// # Errors
    ///
    /// Fails when `now` has no valid UTC form or the descriptor is invalid.
    pub fn pending(
        asset_id: Uuid,
        descriptor: GenerationDescriptor,
        lease_token: Uuid,
        now: OffsetDateTime,
    ) -> Result<Self, InvalidGeneratedIdentity> {
        let now = generation_instant(now)?;
        Self::new(
            asset_id,
            descriptor,
            GenerationState::Pending,
            Some(lease_token),
            Some(now),
            Some(now + GENERATION_LEASE),
            now,
            None,
        )
    }

    /// Re-checks the state shape of this intent.
    ///
    /// # Errors
    ///
    /// Fails when a pending intent lacks a valid lease or a terminal intent keeps one.
    pub fn validate(&self) -> Result<(), InvalidGeneratedIdentity> {
        self.descriptor.validate()?;
        match self.state {
            GenerationState::Pending => {
                let (Some(_), Some(start), Some(expires)) = (
                    self.lease_token,
                    self.lease_started_at,
                    self.lease_expires_at,
                ) else {
                    return Err(InvalidGeneratedIdentity);
                };
                if start < self.created_at
                    || expires <= start
                    || expires > start + GENERATION_LEASE
                    || self.finished_at.is_some()
                {
                    return Err(InvalidGeneratedIdentity);
                }
            }
            GenerationState::Completed | GenerationState::Abandoned => {
                let Some(finished) = self.finished_at else {
                    return Err(InvalidGeneratedIdentity);
                };
                if self.lease_token.is_some()
                    || self.lease_started_at.is_some()
                    || self.lease_expires_at.is_some()
                    || finished < self.created_at
                {
                    return Err(InvalidGeneratedIdentity);
                }
            }
        }
        Ok(())
    }
}

/// A reserved asset paired with its generation intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentReservation {
    /// The reserved asset and its document.
    pub reservation: GeneratedReservation,
    /// The intent that classifies the asset.
    pub intent: GenerationIntent,
}

impl IntentReservation {
    /// Pairs a reservation with its intent; the asset ids must match.
    ///
    /// # Errors
    ///
    /// Fails when the intent is invalid or classifies a different asset.
    pub fn new(
        reservation: GeneratedReservation,
        intent: GenerationIntent,
    ) -> Result<Self, InvalidGeneratedIdentity> {
        let claim = Self {
            reservation,
            intent,
        };
        claim.validate()?;
        Ok(claim)
    }

    /// Re-checks the pairing of a claim built by hand:
    /// the intent shape first, then that the intent classifies the reserved asset. The reserved
    /// asset is typed (its fields are private to [`AssetIdentity`]), so the reservation needs no
    /// further check.
    ///
    /// # Errors
    ///
    /// Fails when the intent is invalid or classifies a different asset.
    pub fn validate(&self) -> Result<(), InvalidGeneratedIdentity> {
        self.intent.validate()?;
        if self.reservation.asset.asset_id() != self.intent.asset_id {
            return Err(InvalidGeneratedIdentity);
        }
        Ok(())
    }

    /// The reserved asset identity.
    #[must_use]
    pub const fn asset_id(&self) -> Uuid {
        self.intent.asset_id
    }
}

/// Bounded exact correspondence of bindings and claims.
/// `claims == None` is the explicit unclassified path.
///
/// Each claim is re-checked with [`IntentReservation::validate`] before it is compared with its
/// binding, because the proof fields are public.
///
/// # Errors
///
/// Fails for more than two bindings, a count mismatch, a claim whose shape or asset pairing is
/// invalid, a claim that does not match its binding, a non-pending claim, or claims from
/// different operations.
pub fn validate_binding_intents(
    bindings: &[GeneratedReservation],
    claims: Option<&[IntentReservation]>,
) -> Result<(), InvalidGeneratedIdentity> {
    if bindings.len() > 2 {
        return Err(InvalidGeneratedIdentity);
    }
    let Some(claims) = claims else {
        return Ok(());
    };
    if claims.len() != bindings.len() {
        return Err(InvalidGeneratedIdentity);
    }
    let mut operation: Option<Uuid> = None;
    for (binding, claim) in bindings.iter().zip(claims) {
        claim.validate()?;
        if &claim.reservation != binding || claim.intent.state != GenerationState::Pending {
            return Err(InvalidGeneratedIdentity);
        }
        if operation.is_some_and(|expected| expected != claim.intent.descriptor.operation_id) {
            return Err(InvalidGeneratedIdentity);
        }
        operation = Some(claim.intent.descriptor.operation_id);
    }
    Ok(())
}
