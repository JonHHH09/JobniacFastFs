//! Personal details in PostgreSQL.
//!
//! The write owns the shared profile compare-and-set first and then upserts only the editable
//! columns, so the photo reference, the creation instant and the children are never touched. The
//! stored `updated_at` is clamped so it never moves backward.

use jobniac_core::errors::AppError;
use jobniac_core::profiles::personal_details::{
    PersonalDetailsChange, PersonalDetailsEnvelope, PersonalDetailsRecord,
};
use jobniac_text::java::java_strip;
use time::OffsetDateTime;
use tokio_postgres::Client;
use uuid::Uuid;

use super::{col, db, unavailable};

/// The profile compare-and-set. `GREATEST` keeps the stored instant
/// monotonic.
const UPDATE_PROFILE_SQL: &str = r#"
                UPDATE profile.profiles
                SET revision = $1, updated_at = GREATEST($2, created_at, updated_at)
                WHERE id = $3 AND revision = $4
                RETURNING updated_at
                "#;

/// The details upsert. The photo reference stays as it is, and the
/// stored `updated_at` is clamped against the row's own timestamps.
const UPSERT_DETAILS_SQL: &str = r#"
                INSERT INTO profile.profile_personal_details
                    (profile_id, date_of_birth, nationality, photo_document_id,
                     created_at, updated_at)
                VALUES ($1, $2, $3, NULL, $4, $5)
                ON CONFLICT (profile_id) DO UPDATE
                SET date_of_birth = EXCLUDED.date_of_birth,
                    nationality = EXCLUDED.nationality,
                    updated_at = GREATEST(
                        EXCLUDED.updated_at, profile.profile_personal_details.created_at,
                        profile.profile_personal_details.updated_at
                    )
                "#;

/// The load query: the root revision and the optional details row.
const LOAD_SQL: &str = r#"
        SELECT p.id AS "profileId", p.revision AS "profileRevision",
               p.updated_at AS "profileUpdatedAt", d.profile_id AS "detailsProfileId",
               d.date_of_birth AS "dateOfBirth", d.nationality,
               d.photo_document_id AS "photoDocumentId", d.created_at AS "createdAt",
               d.updated_at AS "updatedAt"
        FROM profile.profiles AS p
        LEFT JOIN profile.profile_personal_details AS d ON d.profile_id = p.id
        WHERE p.id = $1
        "#;

/// Reads the envelope in a read-only scope, which begins at REPEATABLE READ, so the revision and
/// the details row come from one snapshot.
pub(crate) async fn get_read_only(
    client: &Client,
    profile_id: Uuid,
) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
    load(client, profile_id).await
}

/// Stores the personal details when the profile revision matches. Returns `None` on a mismatch,
/// with nothing written.
pub(crate) async fn update(
    client: &Client,
    change: &PersonalDetailsChange,
) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
    let rows = db(client
        .query(
            UPDATE_PROFILE_SQL,
            &[
                &change.new_revision,
                &change.updated_at,
                &change.profile_id,
                &change.expected_revision,
            ],
        )
        .await)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let stamp: OffsetDateTime = col(row, "updated_at")?;
    db(client
        .execute(
            UPSERT_DETAILS_SQL,
            &[
                &change.profile_id,
                &change.date_of_birth,
                &change.nationality,
                &stamp,
                &stamp,
            ],
        )
        .await)?;
    load(client, change.profile_id)
        .await?
        .ok_or_else(unavailable)
        .map(Some)
}

/// The envelope of a profile, or `None` when the profile is missing.
async fn load(
    client: &Client,
    profile_id: Uuid,
) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
    let rows = db(client.query(LOAD_SQL, &[&profile_id]).await)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let details_profile: Option<Uuid> = col(row, "detailsProfileId")?;
    let personal_details = match details_profile {
        None => None,
        Some(details_profile_id) => {
            let nationality: Option<String> = col(row, "nationality")?;
            let nationality = nationality.and_then(|text| {
                let stripped = java_strip(&text);
                (!stripped.is_empty()).then(|| stripped.to_owned())
            });
            let created_at: Option<OffsetDateTime> = col(row, "createdAt")?;
            let updated_at: Option<OffsetDateTime> = col(row, "updatedAt")?;
            Some(PersonalDetailsRecord {
                profile_id: details_profile_id,
                date_of_birth: col(row, "dateOfBirth")?,
                nationality,
                photo_document_id: col(row, "photoDocumentId")?,
                created_at: created_at.ok_or(AppError::Internal)?,
                updated_at: updated_at.ok_or(AppError::Internal)?,
            })
        }
    };
    Ok(Some(PersonalDetailsEnvelope {
        profile_id: col(row, "profileId")?,
        profile_revision: col(row, "profileRevision")?,
        profile_updated_at: col(row, "profileUpdatedAt")?,
        personal_details,
    }))
}
