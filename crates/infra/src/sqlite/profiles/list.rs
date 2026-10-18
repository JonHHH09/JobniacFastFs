//! Keyset listing of profile identities.
//!
//! The first page fixes a created-at watermark (`snapshot_at`) that later pages reuse, so rows
//! created after the first page do not shift the listing. The SQL selects no resume text and no
//! embedding. The page is built by the core `keyset_page` from `limit + 1` rows.

use jobniac_core::clock::truncate_to_micros;
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use jobniac_core::profiles::model::ProfileIdentity;
use rusqlite::Connection;
use time::OffsetDateTime;

use super::{col, query_rows};
use crate::sqlite::codec::{Id, Instant};

/// The profile listing query. The binds are the snapshot watermark, the anchor's created-at, its
/// identifier and the fetch limit, in that order. The anchor binds are NULL on the first page.
const LIST_PROFILES_SQL: &str = r#"
SELECT profile.id, profile.full_name, profile.email, profile.summary,
       profile.created_at, profile.updated_at, profile.revision
FROM profile_profiles AS profile
WHERE profile.created_at <= ?1
  AND (
      ?2 IS NULL
      OR profile.created_at < ?2
      OR (
          profile.created_at = ?2
          AND profile.id > ?3
      )
  )
ORDER BY profile.created_at DESC, profile.id ASC
LIMIT ?4
"#;

/// Reads one bounded page of identities for `request`.
pub(crate) fn list_profiles(
    conn: &Connection,
    request: &PageRequest<'_>,
) -> Result<Page<ProfileIdentity>, AppError> {
    let anchor = request.cursor();
    // The first page fixes the watermark at the current instant (the old `CURRENT_TIMESTAMP`).
    let snapshot_at = anchor.map_or_else(
        || truncate_to_micros(OffsetDateTime::now_utc()),
        |cursor| cursor.snapshot_at,
    );
    let anchor_created_at = anchor.map(|cursor| Instant(cursor.created_at));
    let anchor_id = anchor.map(|cursor| Id(cursor.id));
    let fetch = i64::try_from(request.limit())
        .map_err(|_| AppError::Internal)?
        .saturating_add(1);
    let identities = query_rows(
        conn,
        LIST_PROFILES_SQL,
        &[
            &Instant(snapshot_at),
            &anchor_created_at,
            &anchor_id,
            &fetch,
        ],
        |row| {
            Ok(ProfileIdentity::new(
                col::<Id>(row, "id")?.0,
                col(row, "full_name")?,
                col(row, "email")?,
                col(row, "summary")?,
                col::<Instant>(row, "created_at")?.0,
                col::<Instant>(row, "updated_at")?.0,
                col(row, "revision")?,
            ))
        },
    )?;
    keyset_page(
        request,
        identities,
        snapshot_at,
        |identity| identity.created_at,
        |identity| identity.id,
    )
}
