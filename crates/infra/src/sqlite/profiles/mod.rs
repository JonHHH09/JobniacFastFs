//! SQLite adapters of the profile ports.
//!
//! One [`SqliteProfileRepository`] implements the four repository ports of
//! `jobniac_core::profiles::repository` over the explicit transaction handle of
//! [`SqliteTransaction`]. [`SqliteUnitOfWork`](crate::sqlite::unit_of_work::SqliteUnitOfWork)
//! implements the core `UnitOfWork` port itself, so the core use cases run their repository calls
//! in its scopes.
//!
//! The SQL uses `?n` binds and the prefixed table names of `sql/sqlite/V0001__baseline.sql`
//! (`profile.profiles` is `profile_profiles`). Every function takes the transaction's
//! [`Connection`] and runs through [`with_connection`].
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

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::pagination::{Page, PageRequest, SearchCandidates};
use jobniac_core::profiles::model::{ProfileGraph, ProfileIdentity};
use jobniac_core::profiles::personal_details::{PersonalDetailsChange, PersonalDetailsEnvelope};
use jobniac_core::profiles::repository::{
    PersonalDetailsRepository, ProfileListRepository, ProfileRepository, ProfileSearchRepository,
    ProjectChange, ProjectPatchResult,
};
use jobniac_core::profiles::search::ProfileSearchCandidate;
use rusqlite::types::FromSql;
use rusqlite::{Connection, Row, ToSql};
use uuid::Uuid;

use crate::sqlite::error::{SqliteError, details_app_error};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// The repository adapter of every profile port. It holds no state: each call runs on the
/// transaction handle that the use case passes in.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteProfileRepository;

#[async_trait]
impl ProfileRepository for SqliteProfileRepository {
    type Tx = SqliteTransaction;

    async fn get_profile(
        &self,
        tx: &mut SqliteTransaction,
        profile_id: Uuid,
    ) -> Result<Option<ProfileGraph>, AppError> {
        with_connection(tx, |conn| graph::load_read_only(conn, profile_id))
    }

    async fn create_profile(
        &self,
        tx: &mut SqliteTransaction,
        graph: &ProfileGraph,
    ) -> Result<ProfileGraph, AppError> {
        with_connection(tx, |conn| graph::create(conn, graph))
    }

    async fn replace_profile(
        &self,
        tx: &mut SqliteTransaction,
        graph: &ProfileGraph,
        expected_revision: i64,
    ) -> Result<Option<ProfileGraph>, AppError> {
        with_connection(tx, |conn| graph::replace(conn, graph, expected_revision))
    }

    async fn update_project(
        &self,
        tx: &mut SqliteTransaction,
        change: &ProjectChange,
    ) -> Result<Option<ProjectPatchResult>, AppError> {
        with_connection(tx, |conn| graph::update_project(conn, change))
    }
}

#[async_trait]
impl PersonalDetailsRepository for SqliteProfileRepository {
    type Tx = SqliteTransaction;

    async fn get_personal_details(
        &self,
        tx: &mut SqliteTransaction,
        profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        with_connection(tx, |conn| personal::get_read_only(conn, profile_id))
    }

    async fn update_personal_details(
        &self,
        tx: &mut SqliteTransaction,
        change: &PersonalDetailsChange,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        with_connection(tx, |conn| personal::update(conn, change))
    }
}

#[async_trait]
impl ProfileListRepository for SqliteProfileRepository {
    type Tx = SqliteTransaction;

    async fn list_profiles(
        &self,
        tx: &mut SqliteTransaction,
        request: &PageRequest<'_>,
    ) -> Result<Page<ProfileIdentity>, AppError> {
        with_connection(tx, |conn| list::list_profiles(conn, request))
    }
}

#[async_trait]
impl ProfileSearchRepository for SqliteProfileRepository {
    type Tx = SqliteTransaction;

    async fn search_profile_candidates(
        &self,
        tx: &mut SqliteTransaction,
        query_tokens: &[String],
    ) -> Result<SearchCandidates<ProfileSearchCandidate>, AppError> {
        with_connection(tx, |conn| {
            search::search_profile_candidates(conn, query_tokens)
        })
    }
}

/// Runs `work` on the connection of the transaction and returns its result.
///
/// A failure of the connection itself is mapped like a statement failure by [`db`].
pub(crate) fn with_connection<R>(
    tx: &SqliteTransaction,
    work: impl FnOnce(&Connection) -> Result<R, AppError>,
) -> Result<R, AppError> {
    tx.run(|conn| Ok(work(conn))).map_err(details_app_error)?
}

/// Converts a driver result of a profile statement into the core error.
///
/// An integrity violation maps to a persistence constraint error and every other database error to
/// "Local database is unavailable". So only a constraint keeps its name, and every other failure
/// (busy, I/O, malformed SQL) becomes "Local database is unavailable". The engine message never
/// leaves the adapter.
pub(crate) fn db<T>(result: rusqlite::Result<T>) -> Result<T, AppError> {
    result.map_err(|error| details_app_error(SqliteError::from(error)))
}

/// Reads one named column of a row. A missing column or a value that does not decode is
/// corrupted data, so it is an internal error and never "unavailable".
pub(crate) fn col<T: FromSql>(row: &Row<'_>, name: &str) -> Result<T, AppError> {
    row.get::<_, T>(name).map_err(|_| AppError::Internal)
}

/// Executes one statement through the connection's statement cache and returns the changed rows.
pub(crate) fn execute(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<usize, AppError> {
    let mut statement = db(conn.prepare_cached(sql))?;
    db(statement.execute(params))
}

/// Runs one query through the statement cache and maps every row with `map`, keeping the row
/// order.
pub(crate) fn query_rows<T>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
    mut map: impl FnMut(&Row<'_>) -> Result<T, AppError>,
) -> Result<Vec<T>, AppError> {
    let mut statement = db(conn.prepare_cached(sql))?;
    let mut rows = db(statement.query(params))?;
    let mut mapped = Vec::new();
    while let Some(row) = db(rows.next())? {
        mapped.push(map(row)?);
    }
    Ok(mapped)
}

/// The error of a database read that must find a row the same transaction just wrote.
pub(crate) fn unavailable() -> AppError {
    AppError::from(ApplicationError::database_unavailable())
}

/// Narrows a core display order (0 to `i32::MAX`) to the range of the PostgreSQL `integer` column
/// that the profile contract keeps.
pub(crate) fn order_value(value: i64) -> Result<i32, AppError> {
    i32::try_from(value).map_err(|_| AppError::Internal)
}

/// Narrows a core search weight to the range of the PostgreSQL `smallint` column that the profile
/// contract keeps.
pub(crate) fn weight_value(value: i64) -> Result<i16, AppError> {
    i16::try_from(value).map_err(|_| AppError::Internal)
}

/// Rule 13 of `RULES_TO_PORT.md` (`assert_generated_document_attachable`): a document that maps to
/// an owned generated asset can be attached only while that asset is `ACTIVE`. The PostgreSQL
/// guard raised a check violation; `constraint` names the trigger it replaces.
pub(crate) fn assert_generated_document_attachable(
    conn: &Connection,
    document_id: Uuid,
    constraint: &str,
) -> Result<(), AppError> {
    let lifecycles = query_rows(
        conn,
        "SELECT lifecycle FROM jobniac_generated_assets WHERE document_id = ?1",
        &[&crate::sqlite::codec::Id(document_id)],
        |row| col::<String>(row, "lifecycle"),
    )?;
    match lifecycles.first().map(String::as_str) {
        None | Some("ACTIVE") => Ok(()),
        Some(_) => Err(AppError::from(SqliteError::constraint(constraint))),
    }
}
