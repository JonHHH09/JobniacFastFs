//! Read-only keyset listing of profile identities.
//!
//! The cursor is bound to the `profiles` scope and the `all` filter, so a cursor from another
//! listing is refused. The page size comes from the shared cursor codec (default 20, at most 100).

use serde::Serialize;

use crate::errors::AppError;
use crate::pagination::CursorCodec;

use super::model::ProfileIdentity;
use super::repository::{ProfileListRepository, READ_ONLY, UnitOfWork};

/// The list envelope: identities and the cursor of the next page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListPage {
    /// The page of identities.
    pub profiles: Vec<ProfileIdentity>,
    /// The cursor of the next page, or null.
    pub next_cursor: Option<String>,
}

/// Lists profile identities through a repository and a cursor codec.
#[derive(Debug)]
pub struct ProfileListService<U, R> {
    uow: U,
    repo: R,
    codec: CursorCodec,
}

impl<U, R> ProfileListService<U, R>
where
    U: UnitOfWork,
    R: ProfileListRepository<Tx = U::Tx>,
{
    /// A service over a unit of work, a repository and the cursor codec of this process.
    pub fn new(uow: U, repo: R, codec: CursorCodec) -> Self {
        Self { uow, repo, codec }
    }

    /// One page of profile identities.
    ///
    /// # Errors
    ///
    /// The cursor error (`invalid_cursor`) for a bad or foreign cursor, the limit error for a limit
    /// outside 1 to 100, or a repository error.
    pub async fn list_profiles(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<ProfileListPage, AppError> {
        let request = self.codec.page_request(limit, cursor, "profiles", "all")?;
        let page = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.list_profiles(tx, &request).await
            })
            .await?;
        Ok(ProfileListPage {
            profiles: page
                .items
                .into_iter()
                .map(ProfileIdentity::without_private_fields)
                .collect(),
            next_cursor: page.next_cursor,
        })
    }
}
