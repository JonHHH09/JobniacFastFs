//! In-memory doubles of the profile ports, shared by the profile integration tests.
//!
//! The profile store applies the same compare-and-set rules as the real adapter, so the tests assert
//! the same outcomes. Each test binary uses a different subset of the helpers, hence the allowance
//! below.
#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jobniac_core::clock::Clock;
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest, SearchCandidates};
use jobniac_core::profiles::model::{
    ProfileGraph, ProfileIdentity, ProjectRecord, ProjectTechnologyRecord,
};
use jobniac_core::profiles::personal_details::{
    PersonalDetailsChange, PersonalDetailsEnvelope, PersonalDetailsRecord,
};
use jobniac_core::profiles::repository::{
    PersonalDetailsRepository, ProfileListRepository, ProfileRepository, ProfileSearchRepository,
    ProjectChange, ProjectPatchResult, UnitOfWork,
};
use jobniac_core::profiles::search::ProfileSearchCandidate;
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

/// Locks a double's state; a poisoned lock still yields its data (the tests never panic while held).
pub fn locked<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A unit of work whose transaction handle is `()`: the doubles hold their own state.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryUow;

impl UnitOfWork for MemoryUow {
    type Tx = ();

    async fn transaction<T, F>(&self, _options: TransactionOptions, body: F) -> Result<T, AppError>
    where
        F: AsyncFnOnce(&mut ()) -> Result<T, AppError>,
    {
        let mut tx = ();
        body(&mut tx).await
    }
}

/// A clock that returns a scripted sequence of instants, then repeats the last one.
#[derive(Debug)]
pub struct SequenceClock {
    values: Mutex<VecDeque<OffsetDateTime>>,
    last: Mutex<OffsetDateTime>,
}

impl SequenceClock {
    /// A clock that yields `values` in order.
    pub fn new(values: Vec<OffsetDateTime>) -> Self {
        let last = values.last().copied().unwrap_or(OffsetDateTime::UNIX_EPOCH);
        Self {
            values: Mutex::new(values.into()),
            last: Mutex::new(last),
        }
    }
}

impl Clock for SequenceClock {
    fn now(&self) -> OffsetDateTime {
        match locked(&self.values).pop_front() {
            Some(value) => {
                *locked(&self.last) = value;
                value
            }
            None => *locked(&self.last),
        }
    }
}

/// The stored profile graphs and the call counters of the profile repository.
#[derive(Debug, Default)]
pub struct ProfileStore {
    /// Stored graphs by profile identifier.
    pub graphs: Mutex<BTreeMap<Uuid, ProfileGraph>>,
    /// Calls to `create_profile`.
    pub creates: AtomicUsize,
    /// Calls to `replace_profile`.
    pub replaces: AtomicUsize,
    /// Calls to `update_project`.
    pub project_updates: AtomicUsize,
}

/// The profile repository double over a shared [`ProfileStore`]. Clones share the store.
#[derive(Debug, Clone, Default)]
pub struct MemoryProfiles {
    /// The shared store.
    pub store: Arc<ProfileStore>,
}

impl MemoryProfiles {
    /// An empty repository.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds a graph as if it had been created earlier.
    pub fn seed(&self, graph: ProfileGraph) {
        locked(&self.store.graphs).insert(graph.profile.id, graph);
    }

    /// The stored graph of a profile.
    pub fn stored(&self, profile_id: Uuid) -> Option<ProfileGraph> {
        locked(&self.store.graphs).get(&profile_id).cloned()
    }

    /// The number of `create_profile` calls so far.
    pub fn creates(&self) -> usize {
        self.store.creates.load(Ordering::SeqCst)
    }

    /// The number of `replace_profile` calls so far.
    pub fn replaces(&self) -> usize {
        self.store.replaces.load(Ordering::SeqCst)
    }

    /// The number of `update_project` calls so far.
    pub fn project_updates(&self) -> usize {
        self.store.project_updates.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ProfileRepository for MemoryProfiles {
    type Tx = ();

    async fn get_profile(
        &self,
        _tx: &mut (),
        profile_id: Uuid,
    ) -> Result<Option<ProfileGraph>, AppError> {
        Ok(self.stored(profile_id))
    }

    async fn create_profile(
        &self,
        _tx: &mut (),
        graph: &ProfileGraph,
    ) -> Result<ProfileGraph, AppError> {
        self.store.creates.fetch_add(1, Ordering::SeqCst);
        locked(&self.store.graphs).insert(graph.profile.id, graph.clone());
        Ok(graph.clone())
    }

    async fn replace_profile(
        &self,
        _tx: &mut (),
        graph: &ProfileGraph,
        expected_revision: i64,
    ) -> Result<Option<ProfileGraph>, AppError> {
        self.store.replaces.fetch_add(1, Ordering::SeqCst);
        let mut graphs = locked(&self.store.graphs);
        match graphs.get(&graph.profile.id) {
            Some(current) if current.profile.revision == expected_revision => {
                graphs.insert(graph.profile.id, graph.clone());
                Ok(Some(graph.clone()))
            }
            _ => Ok(None),
        }
    }

    async fn update_project(
        &self,
        _tx: &mut (),
        change: &ProjectChange,
    ) -> Result<Option<ProjectPatchResult>, AppError> {
        self.store.project_updates.fetch_add(1, Ordering::SeqCst);
        let mut graphs = locked(&self.store.graphs);
        let Some(graph) = graphs.get(&change.profile_id) else {
            return Ok(None);
        };
        if graph.profile.revision != change.expected_revision {
            return Ok(None);
        }
        let mut updated = graph.clone();
        updated.profile.revision = change.new_revision;
        updated.profile.updated_at = change.updated_at;
        let mut persisted: ProjectRecord = change.project.clone();
        if let Some(replacement) = &change.replacement_technologies {
            persisted.technologies = replacement.clone();
        }
        updated.projects = updated
            .projects
            .iter()
            .map(|old| {
                if old.id == persisted.id {
                    persisted.clone()
                } else {
                    old.clone()
                }
            })
            .collect();
        updated.project_technologies = updated
            .projects
            .iter()
            .flat_map(|project| project.technologies.clone())
            .collect::<Vec<ProjectTechnologyRecord>>();
        let result = ProjectPatchResult {
            project: persisted,
            profile_revision: change.new_revision,
        };
        graphs.insert(change.profile_id, updated);
        Ok(Some(result))
    }
}

/// The state of the personal-details double.
#[derive(Debug, Default)]
pub struct PersonalInner {
    /// The stored envelope; `None` when the profile does not exist.
    pub envelope: Mutex<Option<PersonalDetailsEnvelope>>,
    /// Reads so far.
    pub reads: AtomicUsize,
    /// Writes so far.
    pub writes: AtomicUsize,
    /// When set, the write loses its compare-and-set race (returns no envelope).
    pub lose: Mutex<bool>,
    /// When set, the profile is reported missing.
    pub missing: Mutex<bool>,
}

/// The personal-details repository double. Clones share the state.
#[derive(Debug, Clone, Default)]
pub struct MemoryPersonal {
    /// The shared state.
    pub inner: Arc<PersonalInner>,
}

impl MemoryPersonal {
    /// A repository with a profile at `revision` and no personal-details row.
    pub fn with_profile(profile_id: Uuid, revision: i64, updated_at: OffsetDateTime) -> Self {
        let repo = Self::default();
        repo.set_envelope(PersonalDetailsEnvelope {
            profile_id,
            profile_revision: revision,
            profile_updated_at: updated_at,
            personal_details: None,
        });
        repo
    }

    /// Replaces the stored envelope.
    pub fn set_envelope(&self, envelope: PersonalDetailsEnvelope) {
        *locked(&self.inner.envelope) = Some(envelope);
    }

    /// The stored envelope.
    pub fn current(&self) -> Option<PersonalDetailsEnvelope> {
        locked(&self.inner.envelope).clone()
    }

    /// Reads plus writes so far.
    pub fn io_count(&self) -> usize {
        self.inner.reads.load(Ordering::SeqCst) + self.inner.writes.load(Ordering::SeqCst)
    }

    /// Writes so far.
    pub fn writes(&self) -> usize {
        self.inner.writes.load(Ordering::SeqCst)
    }

    /// Makes the next write lose its compare-and-set race.
    pub fn lose_next_write(&self) {
        *locked(&self.inner.lose) = true;
    }

    /// Reports the profile as missing from now on.
    pub fn mark_missing(&self) {
        *locked(&self.inner.missing) = true;
    }
}

#[async_trait::async_trait]
impl PersonalDetailsRepository for MemoryPersonal {
    type Tx = ();

    async fn get_personal_details(
        &self,
        _tx: &mut (),
        _profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        self.inner.reads.fetch_add(1, Ordering::SeqCst);
        if *locked(&self.inner.missing) {
            return Ok(None);
        }
        Ok(self.current())
    }

    async fn update_personal_details(
        &self,
        _tx: &mut (),
        change: &PersonalDetailsChange,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        self.inner.writes.fetch_add(1, Ordering::SeqCst);
        let lost = *locked(&self.inner.lose);
        let mut slot = locked(&self.inner.envelope);
        let Some(current) = slot.clone() else {
            return Ok(None);
        };
        if lost || current.profile_revision != change.expected_revision {
            return Ok(None);
        }
        let existing = current.personal_details.clone();
        let photo = existing
            .as_ref()
            .and_then(|details| details.photo_document_id);
        let created_at = existing
            .as_ref()
            .map_or(change.updated_at, |details| details.created_at);
        let envelope = PersonalDetailsEnvelope {
            profile_id: change.profile_id,
            profile_revision: change.new_revision,
            profile_updated_at: change.updated_at,
            personal_details: Some(PersonalDetailsRecord {
                profile_id: change.profile_id,
                date_of_birth: change.date_of_birth,
                nationality: change.nationality.clone(),
                photo_document_id: photo,
                created_at,
                updated_at: change.updated_at,
            }),
        };
        *slot = Some(envelope.clone());
        Ok(Some(envelope))
    }
}

/// The state of the profile-list double.
#[derive(Debug)]
pub struct ListInner {
    /// The page to return.
    pub page: Page<ProfileIdentity>,
    /// `(limit, scope, has cursor, fingerprint)` of each request.
    pub requests: Mutex<Vec<(usize, String, bool, String)>>,
    /// The cursor position of the last request, when it had one.
    pub last_position: Mutex<Option<(OffsetDateTime, OffsetDateTime, Uuid)>>,
}

/// The profile-list repository double: it records each request and returns a fixed page. Clones
/// share the recorded state.
#[derive(Debug, Clone)]
pub struct MemoryList {
    /// The shared state.
    pub inner: Arc<ListInner>,
}

impl MemoryList {
    /// A repository that returns `page`.
    pub fn new(page: Page<ProfileIdentity>) -> Self {
        Self {
            inner: Arc::new(ListInner {
                page,
                requests: Mutex::new(Vec::new()),
                last_position: Mutex::new(None),
            }),
        }
    }

    /// The recorded requests.
    pub fn requests(&self) -> Vec<(usize, String, bool, String)> {
        locked(&self.inner.requests).clone()
    }

    /// The cursor position of the last request that had a cursor.
    pub fn last_position(&self) -> Option<(OffsetDateTime, OffsetDateTime, Uuid)> {
        *locked(&self.inner.last_position)
    }
}

#[async_trait::async_trait]
impl ProfileListRepository for MemoryList {
    type Tx = ();

    async fn list_profiles(
        &self,
        _tx: &mut (),
        request: &PageRequest<'_>,
    ) -> Result<Page<ProfileIdentity>, AppError> {
        locked(&self.inner.requests).push((
            request.limit(),
            request.scope().to_owned(),
            request.cursor().is_some(),
            request.fingerprint().to_owned(),
        ));
        if let Some(cursor) = request.cursor() {
            *locked(&self.inner.last_position) =
                Some((cursor.snapshot_at, cursor.created_at, cursor.id));
        }
        Ok(self.inner.page.clone())
    }
}

/// The profile-search repository double: returns fixed candidates and records the query tokens.
#[derive(Debug)]
pub struct MemorySearch {
    /// The candidates to return.
    pub result: SearchCandidates<ProfileSearchCandidate>,
    /// The query token lists received.
    pub queries: Mutex<Vec<Vec<String>>>,
}

impl MemorySearch {
    /// A repository that returns `result`.
    pub fn new(result: SearchCandidates<ProfileSearchCandidate>) -> Self {
        Self {
            result,
            queries: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ProfileSearchRepository for MemorySearch {
    type Tx = ();

    async fn search_profile_candidates(
        &self,
        _tx: &mut (),
        query_tokens: &[String],
    ) -> Result<SearchCandidates<ProfileSearchCandidate>, AppError> {
        locked(&self.queries).push(query_tokens.to_vec());
        Ok(self.result.clone())
    }
}
