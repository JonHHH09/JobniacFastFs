//! Sanitized, read-only PostgreSQL readiness probe.
//!
//! The probe runs `SELECT 1` and checks the PostgreSQL 18 major version in a read-only,
//! time-bounded session. It never returns or logs error text; it reports only an [`ErrorCategory`].

use std::time::Duration;

use async_trait::async_trait;
use jobniac_core::health::{ErrorCategory, HealthProbe, HealthProbeResult};
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::postgres::config::DatabaseConfig;
use crate::postgres::sqlstate;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_OPTIONS: &str = "-c default_transaction_read_only=on \
    -c statement_timeout=2000 \
    -c lock_timeout=1000";
const POSTGRES_MAJOR_VERSION: i64 = 18;
const VERSION_NUMBER_DIVISOR: i64 = 10_000;

/// Readiness probe for the configured local database.
///
/// Without a validated configuration, or while `PGSERVICE` is set, the probe reports
/// `ConnectionUnavailable` and never touches the network.
#[derive(Debug)]
pub struct PostgresHealthProbe {
    config: Option<DatabaseConfig>,
}

impl PostgresHealthProbe {
    /// A probe for `config`; `None` when the database is not configured.
    pub fn new(config: Option<DatabaseConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl HealthProbe for PostgresHealthProbe {
    async fn probe(&self) -> HealthProbeResult {
        match &self.config {
            None => HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable),
            Some(config) if config.libpq_service_set() => {
                HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable)
            }
            Some(config) => probe_with_deadline(config).await,
        }
    }
}

/// Run the whole probe under a 10 second backstop; expiry is reported as `Timeout`.
///
/// The connect and each statement are already bounded (2 second connect timeout,
/// `statement_timeout` of 2 seconds). This outer deadline only guards against a network stall
/// between those steps.
async fn probe_with_deadline(config: &DatabaseConfig) -> HealthProbeResult {
    match tokio::time::timeout(PROBE_TIMEOUT, probe_once(config)).await {
        Ok(result) => result,
        Err(_elapsed) => HealthProbeResult::failed(ErrorCategory::Timeout),
    }
}

async fn probe_once(config: &DatabaseConfig) -> HealthProbeResult {
    // The probe session carries no application_name.
    let pg = config.connect_config(CONNECT_TIMEOUT, SESSION_OPTIONS, None);
    // tokio-postgres applies its connect timeout to the TCP socket only, so the whole connection
    // (startup and authentication included) is bounded here. Expiry is a connection failure, like
    // the 2 second libpq connect timeout.
    let (client, connection) = match tokio::time::timeout(CONNECT_TIMEOUT, pg.connect(NoTls)).await
    {
        Ok(Ok(parts)) => parts,
        Ok(Err(error)) => return HealthProbeResult::failed(categorize_connect(&error)),
        Err(_elapsed) => {
            return HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable);
        }
    };
    // The connection task drives the socket and ends when the client is dropped. Its result is
    // discarded, so no driver error text is kept.
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let outcome = run_queries(&client).await;
    drop(client);
    match outcome {
        Ok(()) => HealthProbeResult::ready(),
        Err(category) => HealthProbeResult::failed(category),
    }
}

/// `SELECT 1` must return `1`, and `SHOW server_version_num` must report major version 18.
async fn run_queries(client: &Client) -> Result<(), ErrorCategory> {
    let select = single_text(client, "SELECT 1").await?;
    if select.as_deref() != Some("1") {
        return Err(ErrorCategory::QueryFailed);
    }
    let version = single_text(client, "SHOW server_version_num").await?;
    check_server_version(version.as_deref())
}

/// Accept `server_version_num` values of major version 18 (180000 to 189999); anything else is a
/// query failure.
fn check_server_version(version_num: Option<&str>) -> Result<(), ErrorCategory> {
    match version_num.map(str::parse::<i64>) {
        Some(Ok(number)) if number / VERSION_NUMBER_DIVISOR == POSTGRES_MAJOR_VERSION => Ok(()),
        _ => Err(ErrorCategory::QueryFailed),
    }
}

/// The text of the only column of a single-row result; `None` for any other shape or a NULL.
///
/// A failed statement is categorized as in [`categorize_query`]. A value that cannot be read as
/// text is a client-side fault of the probe, not a connection failure, so it is a query failure.
async fn single_text(client: &Client, sql: &str) -> Result<Option<String>, ErrorCategory> {
    let messages = client
        .simple_query(sql)
        .await
        .map_err(|error| categorize_query(&error))?;
    let rows: Vec<_> = messages
        .iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(row),
            _ => None,
        })
        .collect();
    let [row] = rows.as_slice() else {
        return Ok(None);
    };
    if row.len() != 1 {
        return Ok(None);
    }
    let value = row.try_get(0).map_err(|_| ErrorCategory::QueryFailed)?;
    Ok(value.map(str::to_owned))
}

/// Which step of the probe failed; connect and query failures are categorized differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connect,
    Query,
}

/// Categorize a failure while opening the session. A server error during startup (for example
/// `3D000` or `53300`) is a connection failure.
fn categorize_connect(error: &tokio_postgres::Error) -> ErrorCategory {
    categorize_code(sqlstate::code(error), Phase::Connect)
}

/// Categorize a failure of `SELECT 1` or `SHOW`, in this order: 57014 is a timeout,
/// class 28 an authentication failure, and any other connection-class error a connection
/// failure. Other database errors are query failures.
fn categorize_query(error: &tokio_postgres::Error) -> ErrorCategory {
    categorize_code(sqlstate::code(error), Phase::Query)
}

/// The category for a failure with the given SQLSTATE, without keeping its text. An error with no
/// SQLSTATE (I/O, closed connection) is a connection failure.
fn categorize_code(code: Option<&str>, phase: Phase) -> ErrorCategory {
    match (code, phase) {
        (Some(sqlstate::QUERY_CANCELED), _) => ErrorCategory::Timeout,
        (Some(code), _) if code.starts_with("28") => ErrorCategory::AuthenticationFailed,
        (Some(_), Phase::Connect) => ErrorCategory::ConnectionUnavailable,
        (Some(code), Phase::Query) if sqlstate::is_operational_class(code) => {
            ErrorCategory::ConnectionUnavailable
        }
        (Some(_), Phase::Query) => ErrorCategory::QueryFailed,
        (None, _) => ErrorCategory::ConnectionUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postgres::config::load_database_config;
    use jobniac_core::env::Environment;
    use std::io::ErrorKind;
    use std::net::TcpListener;

    /// With `PGSERVICE` set, the probe reports `ConnectionUnavailable` before it connects. A
    /// loopback listener records whether any connection arrived.
    #[tokio::test]
    async fn probe_refuses_to_connect_while_libpq_service_is_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port().to_string();
        let config = load_database_config(&Environment::from_pairs([
            ("JOBNIAC_DB_HOST", "127.0.0.1"),
            ("JOBNIAC_DB_PORT", port.as_str()),
            ("JOBNIAC_DB_NAME", "jobniac_service"),
            ("JOBNIAC_DB_USER", "app_user"),
            ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
            ("PGSERVICE", "synthetic-service"),
        ]))
        .ok_or("a configuration with PGSERVICE set must still validate")?;

        let result = PostgresHealthProbe::new(Some(config)).probe().await;
        assert_eq!(
            result,
            HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable)
        );
        match listener.accept() {
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
            Ok(_) => Err("the probe attempted a connection while PGSERVICE was set".into()),
            Err(error) => Err(error.into()),
        }
    }

    #[test]
    fn version_gate_accepts_only_major_version_18() {
        assert_eq!(check_server_version(Some("180000")), Ok(()));
        assert_eq!(check_server_version(Some("189999")), Ok(()));
        for version in [Some("170004"), Some("190000"), Some(""), Some("18x"), None] {
            assert_eq!(
                check_server_version(version),
                Err(ErrorCategory::QueryFailed),
                "{version:?}"
            );
        }
    }

    #[test]
    fn query_failures_follow_the_categories() {
        let cases = [
            (Some("57014"), ErrorCategory::Timeout),
            (Some("28P01"), ErrorCategory::AuthenticationFailed),
            (Some("08006"), ErrorCategory::ConnectionUnavailable),
            (Some("53300"), ErrorCategory::ConnectionUnavailable),
            (Some("57P01"), ErrorCategory::ConnectionUnavailable),
            (Some("40P01"), ErrorCategory::ConnectionUnavailable),
            (Some("42P01"), ErrorCategory::QueryFailed),
            (Some("22012"), ErrorCategory::QueryFailed),
            (Some("25006"), ErrorCategory::QueryFailed),
            (None, ErrorCategory::ConnectionUnavailable),
        ];
        for (code, expected) in cases {
            assert_eq!(categorize_code(code, Phase::Query), expected, "{code:?}");
        }
    }

    #[test]
    fn connect_failures_are_connection_failures_unless_authentication_or_timeout() {
        let cases = [
            (Some("57014"), ErrorCategory::Timeout),
            (Some("28000"), ErrorCategory::AuthenticationFailed),
            (Some("28P01"), ErrorCategory::AuthenticationFailed),
            (Some("3D000"), ErrorCategory::ConnectionUnavailable),
            (Some("53300"), ErrorCategory::ConnectionUnavailable),
            (Some("57P03"), ErrorCategory::ConnectionUnavailable),
            (Some("57P01"), ErrorCategory::ConnectionUnavailable),
            (Some("08006"), ErrorCategory::ConnectionUnavailable),
            (None, ErrorCategory::ConnectionUnavailable),
        ];
        for (code, expected) in cases {
            assert_eq!(categorize_code(code, Phase::Connect), expected, "{code:?}");
        }
    }
}
