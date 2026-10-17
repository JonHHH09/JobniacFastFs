//! Ports of the profile use cases.
//!
//! The use cases never see a connection. Each use case runs its repository calls inside
//! [`UnitOfWork::transaction`], which hands the adapter's transaction handle (`Tx`) to the body and
//! commits or rolls back the scope. The PostgreSQL adapter implements [`UnitOfWork`] over
//! `jobniac_infra::postgres::unit_of_work::PgUnitOfWork::transaction`, with `Tx = PgTransaction`.
//! The transaction bodies are async closures (`AsyncFnOnce`), which the PostgreSQL adapter runs
//! through `PgUnitOfWork::transaction`. The repository ports use `async_trait` so that they stay
//! object-safe.

use std::future::Future;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pagination::{Page, PageRequest, SearchCandidates};
use crate::transactions::TransactionOptions;

use super::model::{ProfileGraph, ProfileIdentity, ProjectRecord, ProjectTechnologyRecord};
use super::personal_details::{PersonalDetailsChange, PersonalDetailsEnvelope};
use super::search::ProfileSearchCandidate;

/// Runs use-case bodies inside one transaction scope (the core face of the unit of work).
///
/// The body is an async closure that receives the transaction handle. The scope commits when the
/// body returns `Ok` and rolls back when it returns `Err`, and the error reaches the caller
/// unchanged. The PostgreSQL adapter implements this over `PgUnitOfWork::transaction`, which takes
/// the same kind of closure, with `Tx = PgTransaction`.
pub trait UnitOfWork: Send + Sync {
    /// The open transaction handle that repository calls take.
    type Tx: Send;

    /// Runs `body` in a scope described by `options`. The returned future is `Send` for every
    /// concrete adapter (auto-trait leakage through the adapter's own future).
    fn transaction<T, F>(
        &self,
        options: TransactionOptions,
        body: F,
    ) -> impl Future<Output = Result<T, AppError>>
    where
        F: AsyncFnOnce(&mut Self::Tx) -> Result<T, AppError>;
}

/// Read-only options of a use case that does not write (`default_transaction_read_only=on`).
pub const READ_ONLY: TransactionOptions = TransactionOptions {
    read_only: true,
    requires_new: false,
};

/// Write options of a use case that writes.
pub const READ_WRITE: TransactionOptions = TransactionOptions {
    read_only: false,
    requires_new: false,
};

/// Persistence port for one normalized profile graph.
#[async_trait]
pub trait ProfileRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// The complete graph of a profile, or `None` when it does not exist.
    async fn get_profile(
        &self,
        tx: &mut Self::Tx,
        profile_id: Uuid,
    ) -> Result<Option<ProfileGraph>, AppError>;

    /// Creates the graph and its search postings. Returns the persisted graph.
    async fn create_profile(
        &self,
        tx: &mut Self::Tx,
        graph: &ProfileGraph,
    ) -> Result<ProfileGraph, AppError>;

    /// Replaces the graph when the stored revision equals `expected_revision`. Returns `None` on a
    /// revision mismatch, so the caller reports a conflict.
    async fn replace_profile(
        &self,
        tx: &mut Self::Tx,
        graph: &ProfileGraph,
        expected_revision: i64,
    ) -> Result<Option<ProfileGraph>, AppError>;

    /// Patches one project under the same revision check. Returns `None` on a mismatch.
    async fn update_project(
        &self,
        tx: &mut Self::Tx,
        change: &ProjectChange,
    ) -> Result<Option<ProjectPatchResult>, AppError>;
}

/// The data of one project patch that the repository writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectChange {
    /// Owning profile.
    pub profile_id: Uuid,
    /// Revision the caller read.
    pub expected_revision: i64,
    /// Revision after the patch.
    pub new_revision: i64,
    /// Instant of the patch.
    pub updated_at: OffsetDateTime,
    /// The patched project (its technologies are the current ones unless replaced).
    pub project: ProjectRecord,
    /// Replacement technologies, when the patch supplied them.
    pub replacement_technologies: Option<Vec<ProjectTechnologyRecord>>,
}

/// The result of a project patch.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPatchResult {
    /// The project after the patch.
    pub project: ProjectRecord,
    /// The profile revision after the patch.
    pub profile_revision: i64,
}

/// Persistence port for the optional personal-details row.
#[async_trait]
pub trait PersonalDetailsRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// The envelope of a profile, or `None` when the profile is missing. An absent optional row is
    /// reported as `personalDetails: None` inside the envelope.
    async fn get_personal_details(
        &self,
        tx: &mut Self::Tx,
        profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError>;

    /// Stores the personal details under the profile-revision check. Returns `None` on a mismatch.
    async fn update_personal_details(
        &self,
        tx: &mut Self::Tx,
        change: &PersonalDetailsChange,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError>;
}

/// Persistence port for one bounded profile identity page.
#[async_trait]
pub trait ProfileListRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// One page of identities for `request`, ordered by creation time then identifier.
    async fn list_profiles(
        &self,
        tx: &mut Self::Tx,
        request: &PageRequest<'_>,
    ) -> Result<Page<ProfileIdentity>, AppError>;
}

/// Port for bounded search over persisted profile postings.
#[async_trait]
pub trait ProfileSearchRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// The bounded candidates that match the query tokens (scores and matched keys included).
    async fn search_profile_candidates(
        &self,
        tx: &mut Self::Tx,
        query_tokens: &[String],
    ) -> Result<SearchCandidates<ProfileSearchCandidate>, AppError>;
}
