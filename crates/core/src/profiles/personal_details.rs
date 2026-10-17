//! Optional personal details: date of birth and nationality, under the profile revision contract.
//!
//! The request must contain exactly `dateOfBirth` and `nationality`; both may be null to clear them.
//! Nationality uses Java `strip` semantics (non-breaking spaces stay), and the raw text is counted
//! against the text preparation budget before anything else. `updatedAt` is clamped to the latest
//! of the clock, the profile's update, and the details' own timestamps, so it never moves backward.

use std::sync::Arc;

use jobniac_text::java::{BudgetField, PreparationBudget, java_strip};
use serde::Serialize;
use serde_json::Value;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::{AppError, validation_error};

use super::repository::{PersonalDetailsRepository, READ_ONLY, READ_WRITE, UnitOfWork};
use super::revision::{next_revision, profile_conflict, profile_not_found};
use super::values::{expected_revision, parse_iso_date, uuid_text};

/// Message of every personal-details validation error.
pub const PERSONAL_MESSAGE: &str = "Invalid personal details request";

/// The personal-details row of a profile, when one exists (`personalDetails` of the envelope).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalDetailsRecord {
    /// Owning profile.
    pub profile_id: Uuid,
    /// Date of birth, `YYYY-MM-DD`, or null.
    #[serde(serialize_with = "super::model::opt_date")]
    pub date_of_birth: Option<Date>,
    /// Nationality, or null.
    pub nationality: Option<String>,
    /// Photo document identifier, kept across writes, or null.
    pub photo_document_id: Option<Uuid>,
    /// Creation instant.
    #[serde(serialize_with = "super::model::instant")]
    pub created_at: OffsetDateTime,
    /// Last update instant.
    #[serde(serialize_with = "super::model::instant")]
    pub updated_at: OffsetDateTime,
}

/// The envelope of a personal-details read or write: `profileId`, `profileRevision`,
/// `profileUpdatedAt`, `personalDetails`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalDetailsEnvelope {
    /// The profile.
    pub profile_id: Uuid,
    /// The profile revision.
    pub profile_revision: i64,
    /// The profile's last update instant.
    #[serde(serialize_with = "super::model::instant")]
    pub profile_updated_at: OffsetDateTime,
    /// The details row, or null when none was ever written.
    pub personal_details: Option<PersonalDetailsRecord>,
}

/// The validated values of one personal-details request.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonalDetailsInput {
    /// Date of birth, or none.
    pub date_of_birth: Option<Date>,
    /// Stripped nationality, or none (blank text is none).
    pub nationality: Option<String>,
}

/// The data of one personal-details write that the repository stores.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonalDetailsChange {
    /// The profile.
    pub profile_id: Uuid,
    /// Revision the caller read.
    pub expected_revision: i64,
    /// Revision after the write.
    pub new_revision: i64,
    /// The clamped update instant.
    pub updated_at: OffsetDateTime,
    /// Date of birth to store.
    pub date_of_birth: Option<Date>,
    /// Nationality to store.
    pub nationality: Option<String>,
}

/// Validates a personal-details request.
///
/// # Errors
///
/// Returns the `Invalid personal details request` error of the first invalid field, or the
/// preparation-limit error when the raw nationality exceeds the text budget.
pub fn canonical_personal_details(request: &Value) -> Result<PersonalDetailsInput, AppError> {
    let shape_error = || {
        invalid(
            "personalDetails",
            "must contain exactly dateOfBirth and nationality",
        )
    };
    let Value::Object(map) = request else {
        return Err(shape_error());
    };
    if map.len() != 2 || !map.contains_key("dateOfBirth") || !map.contains_key("nationality") {
        return Err(shape_error());
    }
    let date_of_birth = match &map["dateOfBirth"] {
        Value::Null => None,
        Value::String(text) => match parse_iso_date(text) {
            Some(date) => Some(date),
            None => {
                return Err(invalid(
                    "dateOfBirth",
                    "must be a canonical ISO date or null",
                ));
            }
        },
        _ => {
            return Err(invalid(
                "dateOfBirth",
                "must be a canonical ISO date or null",
            ));
        }
    };
    let nationality = match &map["nationality"] {
        Value::Null => None,
        Value::String(text) => {
            let mut budget = PreparationBudget::new(BudgetField::Text);
            budget.add_text(Some(text.as_str())).map_err(|error| {
                AppError::from(validation_error(
                    error.message(),
                    Some(error.field().as_str()),
                    Some(error.reason()),
                ))
            })?;
            if text.contains('\0') {
                return Err(invalid("nationality", "must be PostgreSQL UTF-8 text"));
            }
            let stripped = java_strip(text);
            (!stripped.is_empty()).then(|| stripped.to_owned())
        }
        _ => return Err(invalid("nationality", "must be text or null")),
    };
    Ok(PersonalDetailsInput {
        date_of_birth,
        nationality,
    })
}

/// Reads and updates the personal details of a profile.
#[derive(Debug)]
pub struct PersonalDetailsService<U, R> {
    uow: U,
    repo: R,
    clock: Arc<dyn Clock>,
}

impl<U, R> PersonalDetailsService<U, R>
where
    U: UnitOfWork,
    R: PersonalDetailsRepository<Tx = U::Tx>,
{
    /// A service over a unit of work, a repository and a clock.
    pub fn new(uow: U, repo: R, clock: Arc<dyn Clock>) -> Self {
        Self { uow, repo, clock }
    }

    /// The personal-details envelope of a profile.
    ///
    /// # Errors
    ///
    /// Validation of `profileId`, or `not_found` when the profile is missing.
    pub async fn get_personal_details(
        &self,
        profile_id: &str,
    ) -> Result<PersonalDetailsEnvelope, AppError> {
        let parsed = uuid_text(profile_id, "profileId")?;
        self.read(parsed).await
    }

    /// Replaces the personal details when `expected_revision` matches the profile revision.
    ///
    /// # Errors
    ///
    /// Validation of the identifier, revision and request (before any read), `not_found`, or
    /// `conflict` for a stale or maximal revision.
    pub async fn update_personal_details(
        &self,
        profile_id: &str,
        expected_revision_value: &Value,
        request: &Value,
    ) -> Result<PersonalDetailsEnvelope, AppError> {
        let parsed = uuid_text(profile_id, "profileId")?;
        let input = canonical_personal_details(request)?;
        let expected = expected_revision(Some(expected_revision_value))?;
        let current = self.read(parsed).await?;
        if current.profile_revision != expected {
            return Err(profile_conflict(parsed, expected));
        }
        let new_revision = next_revision(parsed, expected)?;
        let mut updated_at = self.clock.now();
        let mut stamps = vec![current.profile_updated_at];
        if let Some(details) = &current.personal_details {
            stamps.push(details.created_at);
            stamps.push(details.updated_at);
        }
        for stamp in stamps {
            if stamp > updated_at {
                updated_at = stamp;
            }
        }
        let change = PersonalDetailsChange {
            profile_id: parsed,
            expected_revision: expected,
            new_revision,
            updated_at,
            date_of_birth: input.date_of_birth,
            nationality: input.nationality,
        };
        let result = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.update_personal_details(tx, &change).await
            })
            .await?;
        result.ok_or_else(|| profile_conflict(parsed, expected))
    }

    /// Reads the envelope in a read-only transaction, or reports the missing profile.
    async fn read(&self, profile_id: Uuid) -> Result<PersonalDetailsEnvelope, AppError> {
        let found = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.get_personal_details(tx, profile_id).await
            })
            .await?;
        found.ok_or_else(|| profile_not_found(profile_id))
    }
}

/// A personal-details validation error.
fn invalid(field: &str, reason: &str) -> AppError {
    AppError::from(validation_error(
        PERSONAL_MESSAGE,
        Some(field),
        Some(reason),
    ))
}
