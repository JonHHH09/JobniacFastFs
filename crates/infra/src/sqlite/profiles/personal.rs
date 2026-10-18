//! Personal details in SQLite.
//!
//! The write owns the shared profile compare-and-set first and then upserts only the editable
//! columns, so the photo reference, the creation instant and the children are never touched. The
//! stored `updated_at` is clamped so it never moves backward.

use jobniac_core::errors::AppError;
use jobniac_core::profiles::personal_details::{
    PersonalDetailsChange, PersonalDetailsEnvelope, PersonalDetailsRecord,
};
use jobniac_text::java::java_strip;
use rusqlite::Connection;
use uuid::Uuid;

use super::{col, execute, query_rows, unavailable};
use crate::sqlite::codec::{Day, Id, Instant};

/// The profile compare-and-set. `max` keeps the stored instant monotonic (fixed-width instants
/// compare as strings).
const UPDATE_PROFILE_SQL: &str = r#"
                UPDATE profile_profiles
                SET revision = ?1, updated_at = max(?2, created_at, updated_at)
                WHERE id = ?3 AND revision = ?4
                RETURNING updated_at
                "#;

/// The details upsert. The photo reference stays as it is, and the
/// stored `updated_at` is clamped against the row's own timestamps.
const UPSERT_DETAILS_SQL: &str = r#"
                INSERT INTO profile_profile_personal_details
                    (profile_id, date_of_birth, nationality, photo_document_id,
                     created_at, updated_at)
                VALUES (?1, ?2, ?3, NULL, ?4, ?5)
                ON CONFLICT (profile_id) DO UPDATE
                SET date_of_birth = excluded.date_of_birth,
                    nationality = excluded.nationality,
                    updated_at = max(
                        excluded.updated_at, profile_profile_personal_details.created_at,
                        profile_profile_personal_details.updated_at
                    )
                "#;

/// The load query: the root revision and the optional details row.
const LOAD_SQL: &str = r#"
        SELECT p.id AS "profileId", p.revision AS "profileRevision",
               p.updated_at AS "profileUpdatedAt", d.profile_id AS "detailsProfileId",
               d.date_of_birth AS "dateOfBirth", d.nationality,
               d.photo_document_id AS "photoDocumentId", d.created_at AS "createdAt",
               d.updated_at AS "updatedAt"
        FROM profile_profiles AS p
        LEFT JOIN profile_profile_personal_details AS d ON d.profile_id = p.id
        WHERE p.id = ?1
        "#;

/// Reads the envelope in a read-only scope, which begins `BEGIN DEFERRED`, so the revision and
/// the details row come from one snapshot.
pub(crate) fn get_read_only(
    conn: &Connection,
    profile_id: Uuid,
) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
    load(conn, profile_id)
}

/// Stores the personal details when the profile revision matches. Returns `None` on a mismatch,
/// with nothing written.
pub(crate) fn update(
    conn: &Connection,
    change: &PersonalDetailsChange,
) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
    let stamps = query_rows(
        conn,
        UPDATE_PROFILE_SQL,
        &[
            &change.new_revision,
            &Instant(change.updated_at),
            &Id(change.profile_id),
            &change.expected_revision,
        ],
        |row| Ok(col::<Instant>(row, "updated_at")?.0),
    )?;
    let Some(stamp) = stamps.first().copied() else {
        return Ok(None);
    };
    execute(
        conn,
        UPSERT_DETAILS_SQL,
        &[
            &Id(change.profile_id),
            &change.date_of_birth.map(Day),
            &change.nationality,
            &Instant(stamp),
            &Instant(stamp),
        ],
    )?;
    load(conn, change.profile_id)?
        .ok_or_else(unavailable)
        .map(Some)
}

/// The envelope of a profile, or `None` when the profile is missing.
fn load(conn: &Connection, profile_id: Uuid) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
    let mut envelopes = query_rows(conn, LOAD_SQL, &[&Id(profile_id)], |row| {
        let details_profile: Option<Id> = col(row, "detailsProfileId")?;
        let personal_details = match details_profile {
            None => None,
            Some(Id(details_profile_id)) => {
                let nationality: Option<String> = col(row, "nationality")?;
                let nationality = nationality.and_then(|text| {
                    let stripped = java_strip(&text);
                    (!stripped.is_empty()).then(|| stripped.to_owned())
                });
                let created_at: Option<Instant> = col(row, "createdAt")?;
                let updated_at: Option<Instant> = col(row, "updatedAt")?;
                Some(PersonalDetailsRecord {
                    profile_id: details_profile_id,
                    date_of_birth: col::<Option<Day>>(row, "dateOfBirth")?.map(|day| day.0),
                    nationality,
                    photo_document_id: col::<Option<Id>>(row, "photoDocumentId")?.map(|id| id.0),
                    created_at: created_at.ok_or(AppError::Internal)?.0,
                    updated_at: updated_at.ok_or(AppError::Internal)?.0,
                })
            }
        };
        Ok(PersonalDetailsEnvelope {
            profile_id: col::<Id>(row, "profileId")?.0,
            profile_revision: col(row, "profileRevision")?,
            profile_updated_at: col::<Instant>(row, "profileUpdatedAt")?.0,
            personal_details,
        })
    })?;
    Ok(envelopes.pop())
}
