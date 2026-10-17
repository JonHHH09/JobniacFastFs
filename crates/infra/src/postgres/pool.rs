//! Bounded pool of application PostgreSQL sessions.
//!
//! Every session is built from a validated [`DatabaseConfig`] (loopback only, see `config.rs`)
//! with the shared application session policy of [`crate::postgres::session`]:
//! `search_path=pg_catalog`, statement timeout 30 s, lock timeout 10 s, idle-in-transaction timeout
//! 120 s, read-write default, connect timeout 5 s and `application_name` `jobniac` (`jobniac-job-search` for the job search pool, see
//! [`PgPool::with_application_name`]). A configuration loaded while `PGSERVICE` is set is refused
//! before any connection.
//!
//! Pool limits:
//! * The number of sessions in use is capped by a semaphore ([`DEFAULT_MAX_CONNECTIONS`]). Each
//!   nested `REQUIRES_NEW` opens a second session.
//! * A caller waits at most [`DEFAULT_ACQUIRE_TIMEOUT`] for a free session; then the database is
//!   reported unavailable. A nested `REQUIRES_NEW` needs a second session, so the cap must be
//!   larger than the deepest nesting that can be active at once.
//! * A whole connect (socket, startup and authentication) is bounded by
//!   [`crate::postgres::session::CONNECT_TIMEOUT`]. `tokio-postgres` applies its connect timeout to
//!   the socket only, so the outer timeout is the one that bounds the whole connect.
//!
//! A [`PooledSession`] is returned to the pool only by its release (crate-internal), and only
//! after the owning transaction has ended and its session settings were reset. A session that was dropped without release (for example
//! because its future was cancelled) is closed, so the server rolls back its open transaction.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::{Client, NoTls};

use crate::postgres::config::DatabaseConfig;
use crate::postgres::session::{APPLICATION_NAME, APPLICATION_OPTIONS, CONNECT_TIMEOUT};
use crate::postgres::unit_of_work::PgError;

/// Default upper bound of application sessions in use at once.
pub const DEFAULT_MAX_CONNECTIONS: usize = 8;

/// How long [`PgPool::acquire`] waits for a free session before it reports the database
/// unavailable.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Idle sessions kept for reuse.
const MAX_IDLE_CONNECTIONS: usize = 4;

/// A bounded pool of application sessions for one validated configuration.
///
/// Cloning is cheap: all clones share the same permits and idle sessions. An unconfigured pool
/// (`None`) fails every acquire with [`PgError::NotConfigured`] before it waits or connects.
#[derive(Clone)]
pub struct PgPool {
    shared: Arc<PoolShared>,
}

struct PoolShared {
    config: Option<DatabaseConfig>,
    application_name: &'static str,
    permits: Arc<Semaphore>,
    acquire_timeout: Duration,
    max_idle: usize,
    idle: Mutex<Vec<Client>>,
    closed: AtomicBool,
}

impl fmt::Debug for PgPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgPool")
            .field("configured", &self.configured())
            .field("idle", &self.idle_count())
            .field("closed", &self.shared.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl PgPool {
    /// A pool with [`DEFAULT_MAX_CONNECTIONS`] sessions and [`DEFAULT_ACQUIRE_TIMEOUT`], whose
    /// sessions carry the application name [`APPLICATION_NAME`].
    pub fn new(config: Option<DatabaseConfig>) -> Self {
        Self::with_limits(config, DEFAULT_MAX_CONNECTIONS, DEFAULT_ACQUIRE_TIMEOUT)
    }

    /// A pool with the default limits whose sessions carry `application_name`. The job search uses
    /// its own pool, so its sessions are named `jobniac-job-search` in `pg_stat_activity`.
    pub fn with_application_name(
        config: Option<DatabaseConfig>,
        application_name: &'static str,
    ) -> Self {
        Self::build(
            config,
            application_name,
            DEFAULT_MAX_CONNECTIONS,
            DEFAULT_ACQUIRE_TIMEOUT,
        )
    }

    /// A pool with explicit limits. `max_connections` below 1 is raised to 1.
    pub fn with_limits(
        config: Option<DatabaseConfig>,
        max_connections: usize,
        acquire_timeout: Duration,
    ) -> Self {
        Self::build(config, APPLICATION_NAME, max_connections, acquire_timeout)
    }

    fn build(
        config: Option<DatabaseConfig>,
        application_name: &'static str,
        max_connections: usize,
        acquire_timeout: Duration,
    ) -> Self {
        let max_connections = max_connections.max(1);
        Self {
            shared: Arc::new(PoolShared {
                config,
                application_name,
                permits: Arc::new(Semaphore::new(max_connections)),
                acquire_timeout,
                max_idle: max_connections.min(MAX_IDLE_CONNECTIONS),
                idle: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Whether the pool has a validated configuration.
    pub fn configured(&self) -> bool {
        self.shared.config.is_some()
    }

    /// Take one session, waiting at most the acquire timeout for a free slot.
    ///
    /// Idle sessions are reused first. Otherwise a new session is connected within the connect
    /// timeout.
    ///
    /// # Errors
    ///
    /// * [`PgError::NotConfigured`] when the pool has no configuration.
    /// * [`PgError::Unavailable`] when the pool is closed, the configuration was loaded while
    ///   `PGSERVICE` was set (refused before any connection), no slot frees up in
    ///   time, or the connection cannot be established. The message never names the cause.
    pub async fn acquire(&self) -> Result<PooledSession, PgError> {
        let shared = &self.shared;
        let config = match shared.config.as_ref() {
            Some(config) => config,
            None => return Err(PgError::NotConfigured),
        };
        if config.libpq_service_set() || shared.closed.load(Ordering::SeqCst) {
            return Err(PgError::Unavailable);
        }
        let permit = match tokio::time::timeout(
            shared.acquire_timeout,
            Arc::clone(&shared.permits).acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) | Err(_) => return Err(PgError::Unavailable),
        };
        let client = match shared.take_idle() {
            Some(client) => client,
            None => connect(config, shared.application_name).await?,
        };
        Ok(PooledSession {
            client,
            shared: Arc::clone(shared),
            permit,
        })
    }

    /// Close the pool: idle sessions are dropped and later releases do not keep their sessions.
    /// Sessions in use stay usable until they are released.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.shared.lock_idle().clear();
    }

    /// Number of idle sessions kept for reuse. Sessions in use are not counted.
    pub fn idle_count(&self) -> usize {
        self.shared.lock_idle().len()
    }
}

impl PoolShared {
    fn lock_idle(&self) -> MutexGuard<'_, Vec<Client>> {
        // A poisoned lock still holds a valid list of sessions, so the pool keeps working.
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The newest idle session that is still open, if any.
    fn take_idle(&self) -> Option<Client> {
        loop {
            let client = self.lock_idle().pop()?;
            if !client.is_closed() {
                return Some(client);
            }
        }
    }

    fn put_idle(&self, client: Client) {
        let mut idle = self.lock_idle();
        if !self.closed.load(Ordering::SeqCst) && idle.len() < self.max_idle {
            idle.push(client);
        }
    }
}

/// Connect one application session from a validated configuration.
async fn connect(config: &DatabaseConfig, application_name: &str) -> Result<Client, PgError> {
    let settings =
        config.connect_config(CONNECT_TIMEOUT, APPLICATION_OPTIONS, Some(application_name));
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, settings.connect(NoTls)).await;
    let (client, connection) = match connected {
        Ok(Ok(parts)) => parts,
        Ok(Err(_)) | Err(_) => return Err(PgError::Unavailable),
    };
    // The connection task ends when the client is dropped, which closes the session. A session that
    // ends for any other reason (for example a server shutdown) is logged by its SQLSTATE class only.
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::warn!(reason = %connection_end_reason(&error), "application session ended");
        }
    });
    Ok(client)
}

/// Why a session ended, without the server text: its SQLSTATE, or a generic connection error.
fn connection_end_reason(error: &tokio_postgres::Error) -> String {
    match error.code() {
        Some(code) => format!("sqlstate {}", code.code()),
        None => "connection error".to_owned(),
    }
}

/// One application session checked out of a [`PgPool`].
///
/// Holds one slot of the pool until it is released or dropped. Use it through the owning
/// transaction; repositories never own a session.
pub struct PooledSession {
    client: Client,
    shared: Arc<PoolShared>,
    permit: OwnedSemaphorePermit,
}

impl fmt::Debug for PooledSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledSession")
            .field("closed", &self.client.is_closed())
            .finish_non_exhaustive()
    }
}

impl PooledSession {
    /// The `tokio-postgres` client of this session.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Return the session to the pool. A session that is closed or was marked not reusable is
    /// closed instead. The pool slot is freed last, so a waiting caller finds the idle session.
    pub(crate) fn release(self, reusable: bool) {
        let Self {
            client,
            shared,
            permit,
        } = self;
        if reusable && !client.is_closed() {
            shared.put_idle(client);
        }
        drop(permit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_pool_reports_its_state() {
        let pool = PgPool::new(None);
        assert!(!pool.configured());
        assert_eq!(pool.idle_count(), 0);
        let rendered = format!("{pool:?}");
        assert!(rendered.contains("configured: false"));
    }

    #[test]
    fn zero_connections_are_raised_to_one() {
        let pool = PgPool::with_limits(None, 0, DEFAULT_ACQUIRE_TIMEOUT);
        assert_eq!(pool.shared.permits.available_permits(), 1);
        assert_eq!(pool.shared.max_idle, 1);
    }

    #[test]
    fn idle_cap_is_the_smaller_of_the_limit_and_four() {
        let pool = PgPool::new(None);
        assert_eq!(
            pool.shared.permits.available_permits(),
            DEFAULT_MAX_CONNECTIONS
        );
        assert_eq!(pool.shared.max_idle, MAX_IDLE_CONNECTIONS);
    }

    #[tokio::test]
    async fn acquire_without_configuration_fails_before_waiting() {
        let pool = PgPool::with_limits(None, 1, Duration::from_millis(1));
        let outcome = pool.acquire().await;
        assert!(matches!(outcome, Err(PgError::NotConfigured)));
        assert_eq!(pool.shared.permits.available_permits(), 1);
    }
}
