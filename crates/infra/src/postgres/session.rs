//! Application session settings shared by the PostgreSQL adapters.
//!
//! The migration and probe sessions have their own settings in their modules.

use std::time::Duration;

/// Connect timeout of application and migration sessions.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Session options of application connections. `search_path` is pinned, so all application SQL
/// must be schema-qualified.
pub const APPLICATION_OPTIONS: &str = "-c search_path=pg_catalog \
    -c statement_timeout=30000 \
    -c lock_timeout=10000 \
    -c idle_in_transaction_session_timeout=120000 \
    -c default_transaction_read_only=off";

/// `application_name` of application connections.
pub const APPLICATION_NAME: &str = "jobniac";

/// `application_name` of the job search connections, so that search
/// sessions can be told apart in `pg_stat_activity`.
pub const JOB_SEARCH_APPLICATION_NAME: &str = "jobniac-job-search";
