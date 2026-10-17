//! PostgreSQL adapters of the profile ports.
//!
//! One [`PgProfileRepository`] implements the four repository ports of
//! `jobniac_core::profiles::repository` over the explicit transaction handle of
//! [`PgTransaction`]. [`PgUnitOfWork`] implements the core [`UnitOfWork`] port, so the core use
//! cases run their repository calls in its scopes.
//!
//! The SQL uses `$n` binds. Schema qualification is explicit because the session pins
//! `search_path` to `pg_catalog`.
//!
//! * [`graph`]: the profile aggregate (load, create, replace, project patch, search postings).
//! * [`personal`]: the optional personal-details row under the profile revision check.
//! * [`list`]: the bounded keyset listing of identities.
//! * [`search`]: the bounded candidate search over the posting index.

pub mod graph;
pub mod list;
pub mod pdf;
pub mod personal;
pub mod search;

use std::future::Future;

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::pagination::{Page, PageRequest, SearchCandidates};
use jobniac_core::profiles::model::{ProfileGraph, ProfileIdentity};
use jobniac_core::profiles::personal_details::{PersonalDetailsChange, PersonalDetailsEnvelope};
use jobniac_core::profiles::repository::{
    PersonalDetailsRepository, ProfileListRepository, ProfileRepository, ProfileSearchRepository,
    ProjectChange, ProjectPatchResult, UnitOfWork,
};
use jobniac_core::profiles::search::ProfileSearchCandidate;
use jobniac_core::transactions::TransactionOptions;
use tokio_postgres::Row;
use tokio_postgres::types::FromSql;
use uuid::Uuid;

use crate::postgres::unit_of_work::{PgError, PgTransaction, PgUnitOfWork};

/// The repository adapter of every profile port. It holds no state: each call runs on the
/// transaction handle that the use case passes in.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgProfileRepository;

impl UnitOfWork for PgUnitOfWork {
    type Tx = PgTransaction;

    fn transaction<T, F>(
        &self,
        options: TransactionOptions,
        body: F,
    ) -> impl Future<Output = Result<T, AppError>>
    where
        F: AsyncFnOnce(&mut PgTransaction) -> Result<T, AppError>,
    {
        PgUnitOfWork::transaction::<T, AppError>(self, options, body)
    }
}

#[async_trait]
impl ProfileRepository for PgProfileRepository {
    type Tx = PgTransaction;

    async fn get_profile(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<Option<ProfileGraph>, AppError> {
        graph::load_read_only(tx.client(), profile_id).await
    }

    async fn create_profile(
        &self,
        tx: &mut PgTransaction,
        graph: &ProfileGraph,
    ) -> Result<ProfileGraph, AppError> {
        graph::create(tx.client(), graph).await
    }

    async fn replace_profile(
        &self,
        tx: &mut PgTransaction,
        graph: &ProfileGraph,
        expected_revision: i64,
    ) -> Result<Option<ProfileGraph>, AppError> {
        graph::replace(tx.client(), graph, expected_revision).await
    }

    async fn update_project(
        &self,
        tx: &mut PgTransaction,
        change: &ProjectChange,
    ) -> Result<Option<ProjectPatchResult>, AppError> {
        graph::update_project(tx.client(), change).await
    }
}

#[async_trait]
impl PersonalDetailsRepository for PgProfileRepository {
    type Tx = PgTransaction;

    async fn get_personal_details(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        personal::get_read_only(tx.client(), profile_id).await
    }

    async fn update_personal_details(
        &self,
        tx: &mut PgTransaction,
        change: &PersonalDetailsChange,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        personal::update(tx.client(), change).await
    }
}

#[async_trait]
impl ProfileListRepository for PgProfileRepository {
    type Tx = PgTransaction;

    async fn list_profiles(
        &self,
        tx: &mut PgTransaction,
        request: &PageRequest<'_>,
    ) -> Result<Page<ProfileIdentity>, AppError> {
        list::list_profiles(tx.client(), request).await
    }
}

#[async_trait]
impl ProfileSearchRepository for PgProfileRepository {
    type Tx = PgTransaction;

    async fn search_profile_candidates(
        &self,
        tx: &mut PgTransaction,
        query_tokens: &[String],
    ) -> Result<SearchCandidates<ProfileSearchCandidate>, AppError> {
        search::search_profile_candidates(tx.client(), query_tokens).await
    }
}

/// Converts a driver result of a profile statement into the core error.
///
/// An integrity violation (SQLSTATE class 23) maps to a validation error and every other database
/// error to "Local database is unavailable". So only class 23 keeps its constraint name, and every
/// other database failure (timeouts, lock timeouts, class 22, 42, 25, P0001 and the rest) becomes
/// "Local database is unavailable". The server message never leaves the adapter. The generic unit-of-work mapping in `unit_of_work.rs` is not used here.
pub(crate) fn db<T>(result: Result<T, tokio_postgres::Error>) -> Result<T, AppError> {
    result.map_err(|error| match PgError::from(error) {
        constraint @ PgError::Constraint { .. } => AppError::from(constraint),
        _ => AppError::from(PgError::Unavailable),
    })
}

/// Reads one named column of a row. A missing column or a value that does not decode is
/// corrupted data, so it is an internal error and never "unavailable".
pub(crate) fn col<'a, T>(row: &'a Row, name: &str) -> Result<T, AppError>
where
    T: FromSql<'a>,
{
    row.try_get::<_, T>(name).map_err(|_| AppError::Internal)
}

/// Maps every row of a result with `map`, keeping the row order.
pub(crate) fn map_rows<T>(
    rows: &[Row],
    map: impl Fn(&Row) -> Result<T, AppError>,
) -> Result<Vec<T>, AppError> {
    rows.iter().map(map).collect()
}

/// The error of a database read that must find a row the same transaction just wrote.
pub(crate) fn unavailable() -> AppError {
    AppError::from(ApplicationError::database_unavailable())
}

/// Narrows a core display order (0 to `i32::MAX`) to the `integer` column type.
pub(crate) fn order_value(value: i64) -> Result<i32, AppError> {
    i32::try_from(value).map_err(|_| AppError::Internal)
}

/// Narrows a core search weight to the `smallint` column type.
pub(crate) fn weight_value(value: i64) -> Result<i16, AppError> {
    i16::try_from(value).map_err(|_| AppError::Internal)
}
