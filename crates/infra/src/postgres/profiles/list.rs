//! Keyset listing of profile identities.
//!
//! The first page fixes a created-at watermark (`snapshot_at`) that later pages reuse, so rows
//! created after the first page do not shift the listing. The SQL selects no resume text and no
//! embedding. The page is built by the core `keyset_page` from `limit + 1` rows.

use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use jobniac_core::profiles::model::ProfileIdentity;
use time::OffsetDateTime;

use super::{col, db};
use tokio_postgres::Client;

/// The profile listing query. The six binds are the anchor's snapshot, its created-at (three
/// times), its identifier and the fetch limit, in that order.
const LIST_PROFILES_SQL: &str = r#"
WITH bounds AS (
    SELECT COALESCE($1::timestamptz, CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT profile.id, profile.full_name, profile.email, profile.summary,
       profile.created_at, profile.updated_at, profile.revision, bounds.snapshot_at
FROM profile.profiles AS profile
CROSS JOIN bounds
WHERE profile.created_at <= bounds.snapshot_at
  AND (
      $2::timestamptz IS NULL
      OR profile.created_at < $3::timestamptz
      OR (
          profile.created_at = $4::timestamptz
          AND profile.id > $5::uuid
      )
  )
ORDER BY profile.created_at DESC, profile.id ASC
LIMIT $6
"#;

/// Reads one bounded page of identities for `request`.
pub(crate) async fn list_profiles(
    client: &Client,
    request: &PageRequest<'_>,
) -> Result<Page<ProfileIdentity>, AppError> {
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let anchor_created_at = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = i64::try_from(request.limit())
        .map_err(|_| AppError::Internal)?
        .saturating_add(1);
    let rows = db(client
        .query(
            LIST_PROFILES_SQL,
            &[
                &snapshot,
                &anchor_created_at,
                &anchor_created_at,
                &anchor_created_at,
                &anchor_id,
                &fetch,
            ],
        )
        .await)?;
    let mut identities = Vec::with_capacity(rows.len());
    let mut snapshot_at = None;
    for row in &rows {
        if snapshot_at.is_none() {
            snapshot_at = Some(col::<OffsetDateTime>(row, "snapshot_at")?);
        }
        identities.push(ProfileIdentity::new(
            col(row, "id")?,
            col(row, "full_name")?,
            col(row, "email")?,
            col(row, "summary")?,
            col(row, "created_at")?,
            col(row, "updated_at")?,
            col(row, "revision")?,
        ));
    }
    // The first row's watermark, else the anchor's, else the current instant. Without
    // rows and without an anchor no cursor is issued, so the last fallback is never signed.
    let snapshot_at = snapshot_at
        .or(snapshot)
        .unwrap_or_else(OffsetDateTime::now_utc);
    keyset_page(
        request,
        identities,
        snapshot_at,
        |identity| identity.created_at,
        |identity| identity.id,
    )
}
