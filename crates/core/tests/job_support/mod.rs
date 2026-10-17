//! In-memory doubles of the job ports, shared by the job test binaries.
//!
//! Stored aggregates are copies, every repository call is logged in order (so a test can assert that
//! validation ran before any repository access), and the compare-and-set rules match the PostgreSQL
//! adapter's contract.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::jobs::{IdSource, JobAggregate, JobRecord, JobRepository, JobSearchRepository};
use jobniac_core::pagination::{Page, PageRequest, SearchCandidates, keyset_page};
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::TransactionOptions;
use uuid::Uuid;

/// Locks a double's state; a poisoned lock still yields its data.
pub fn locked<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A unit of work whose transaction handle is `()`.
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

/// Identifiers `UUID(int=n)` for the n-th request, as the golden vectors' counter produces them.
#[derive(Debug, Default)]
pub struct CounterIds {
    next: AtomicU64,
}

impl CounterIds {
    /// Restarts the counter, so the next identifier is `UUID(int=1)`.
    pub fn reset(&self) {
        self.next.store(0, Ordering::SeqCst);
    }
}

impl IdSource for CounterIds {
    fn next_id(&self) -> Uuid {
        let value = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        Uuid::from_u128(u128::from(value))
    }
}

/// A fixed clock at `instant` (RFC 3339 text in the `Instant.toString()` form).
pub fn fixed_clock(instant: &str) -> Arc<FixedClock> {
    let parsed = match jobniac_core::clock::parse_instant(instant) {
        Ok(value) => value,
        Err(error) => panic!("test instant does not parse: {error}"),
    };
    Arc::new(FixedClock::new(parsed))
}

/// Stored aggregates, the call log and the race switch of the job repository.
#[derive(Debug, Default)]
pub struct JobStore {
    /// Stored aggregates by job identifier.
    pub graphs: Mutex<BTreeMap<Uuid, JobAggregate>>,
    /// Repository calls, in order.
    pub calls: Mutex<Vec<&'static str>>,
    /// The number of upcoming updates that lose the compare-and-set race.
    pub races: Mutex<usize>,
    /// The `(scope, fingerprint, limit, has_cursor)` of every list call.
    pub lists: Mutex<Vec<(String, String, usize, bool)>>,
}

/// The job repository double. Clones share the store.
#[derive(Debug, Clone, Default)]
pub struct MemoryJobs {
    /// The shared store.
    pub store: Arc<JobStore>,
}

impl MemoryJobs {
    /// An empty repository.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds an aggregate as if it had been written earlier.
    pub fn seed(&self, graph: JobAggregate) {
        locked(&self.store.graphs).insert(graph.job.id, graph);
    }

    /// The stored aggregate of a job.
    pub fn stored(&self, job_id: Uuid) -> Option<JobAggregate> {
        locked(&self.store.graphs).get(&job_id).cloned()
    }

    /// The repository calls so far, cleared.
    pub fn take_calls(&self) -> Vec<&'static str> {
        std::mem::take(&mut *locked(&self.store.calls))
    }

    /// Makes the next `count` updates lose the compare-and-set race.
    pub fn lose_races(&self, count: usize) {
        *locked(&self.store.races) = count;
    }

    fn log(&self, call: &'static str) {
        locked(&self.store.calls).push(call);
    }

    fn by_fingerprint(&self, fingerprint: &str) -> Option<JobAggregate> {
        locked(&self.store.graphs)
            .values()
            .find(|graph| graph.job.canonical_fingerprint == fingerprint)
            .cloned()
    }
}

#[async_trait]
impl JobRepository for MemoryJobs {
    type Tx = ();

    async fn find_by_text_hash(
        &self,
        _tx: &mut (),
        input_text_hash: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        self.log("find_by_text_hash");
        Ok(locked(&self.store.graphs)
            .values()
            .find(|graph| {
                graph
                    .text_ingestion
                    .as_ref()
                    .is_some_and(|ingestion| ingestion.input_text_hash == input_text_hash)
            })
            .cloned())
    }

    async fn get_job(&self, _tx: &mut (), job_id: Uuid) -> Result<Option<JobAggregate>, AppError> {
        self.log("get_job");
        Ok(self.stored(job_id))
    }

    async fn list_jobs(
        &self,
        _tx: &mut (),
        request: &PageRequest<'_>,
    ) -> Result<Page<JobRecord>, AppError> {
        self.log("list_jobs");
        locked(&self.store.lists).push((
            request.scope().to_owned(),
            request.fingerprint().to_owned(),
            request.limit(),
            request.cursor().is_some(),
        ));
        let mut rows: Vec<JobRecord> = locked(&self.store.graphs)
            .values()
            .map(|graph| graph.job.clone())
            .collect();
        rows.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        if let Some(cursor) = request.cursor() {
            rows.retain(|row| (row.created_at, row.id) > (cursor.created_at, cursor.id));
        }
        let snapshot = rows
            .last()
            .map_or(cursor_snapshot(request), |row| row.created_at);
        keyset_page(request, rows, snapshot, |row| row.created_at, |row| row.id)
    }

    async fn create_text_job(
        &self,
        _tx: &mut (),
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        self.log("create_text_job");
        if let Some(existing) = self.by_fingerprint(&graph.job.canonical_fingerprint) {
            return Ok((existing, false));
        }
        locked(&self.store.graphs).insert(graph.job.id, graph.clone());
        Ok((graph.clone(), true))
    }

    async fn find_by_canonical_fingerprint(
        &self,
        _tx: &mut (),
        fingerprint: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        self.log("find_by_canonical_fingerprint");
        Ok(self.by_fingerprint(fingerprint))
    }

    async fn update_job_aggregate(
        &self,
        _tx: &mut (),
        graph: &JobAggregate,
        expected_revision: i64,
    ) -> Result<Option<JobAggregate>, AppError> {
        self.log("update_job_aggregate");
        {
            let mut races = locked(&self.store.races);
            if *races > 0 {
                *races -= 1;
                if let Some(stored) = locked(&self.store.graphs).get_mut(&graph.job.id) {
                    stored.job.revision += 1;
                }
            }
        }
        let mut graphs = locked(&self.store.graphs);
        match graphs.get(&graph.job.id) {
            Some(stored) if stored.job.revision == expected_revision => {
                graphs.insert(graph.job.id, graph.clone());
                Ok(Some(graph.clone()))
            }
            _ => Ok(None),
        }
    }
}

/// The snapshot instant of an empty page: the cursor's snapshot when one was given.
fn cursor_snapshot(request: &PageRequest<'_>) -> time::OffsetDateTime {
    request
        .cursor()
        .map_or(time::OffsetDateTime::UNIX_EPOCH, |cursor| {
            cursor.snapshot_at
        })
}

/// The search repository double: returns the scripted candidates and logs the arguments.
#[derive(Debug, Default)]
pub struct SearchStore {
    /// The candidates to return.
    pub candidates: Mutex<Option<SearchCandidates<JobAggregate>>>,
    /// Repository calls, in order.
    pub calls: Mutex<Vec<&'static str>>,
    /// The `(query tokens, limit)` of the last search.
    pub arguments: Mutex<Option<(Vec<String>, usize)>>,
}

/// The search repository double. Clones share the store.
#[derive(Debug, Clone, Default)]
pub struct MemorySearch {
    /// The shared store.
    pub store: Arc<SearchStore>,
}

impl MemorySearch {
    /// A repository that returns `candidates` for every search.
    pub fn with_candidates(candidates: SearchCandidates<JobAggregate>) -> Self {
        let search = Self::default();
        *locked(&search.store.candidates) = Some(candidates);
        search
    }
}

#[async_trait]
impl JobSearchRepository for MemorySearch {
    type Tx = ();

    async fn search_job_candidates(
        &self,
        _tx: &mut (),
        query_tokens: &[String],
        limit: usize,
    ) -> Result<SearchCandidates<JobAggregate>, AppError> {
        locked(&self.store.calls).push("search_job_candidates");
        *locked(&self.store.arguments) = Some((query_tokens.to_vec(), limit));
        let scripted = locked(&self.store.candidates).clone();
        match scripted {
            Some(candidates) => Ok(candidates),
            None => SearchCandidates::new(0, false, Vec::new()),
        }
    }
}
