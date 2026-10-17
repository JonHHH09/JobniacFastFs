//! Access to the disposable PostgreSQL 18 cluster that `pg-test` owns for one test run.
//!
//! `pg-test` exports the cluster's loopback port and bootstrap superuser through
//! `JOBNIAC_TEST_PG_PORT`, `JOBNIAC_TEST_PG_USER` and `JOBNIAC_TEST_PG_PASSWORD`. Each
//! [`TestServer::fresh_database`] call creates an empty database that is dropped again when the
//! returned [`TestDatabase`] goes out of scope.

use std::env;
use std::fmt;
use std::sync::mpsc;
use std::time::Duration;

use jobniac_core::env::Environment;
use tokio_postgres::{Client, Config, NoTls};

const PORT_VAR: &str = "JOBNIAC_TEST_PG_PORT";
const USER_VAR: &str = "JOBNIAC_TEST_PG_USER";
const PASSWORD_VAR: &str = "JOBNIAC_TEST_PG_PASSWORD";
const HOST: &str = "127.0.0.1";
const DATABASE_PREFIX: &str = "jobniac_test_";
const ADMIN_DATABASE: &str = "postgres";
/// Bounds every admin connection, like a libpq `connect_timeout`. tokio-postgres
/// applies its connect timeout to the socket only, so the whole connect is bounded here.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the drop waits for its cleanup thread before reporting a leaked database.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Connection values of the disposable cluster owned by one `pg-test` run.
///
/// `Debug` never prints the password.
#[derive(Clone)]
pub struct TestServer {
    host: &'static str,
    port: u16,
    superuser: String,
    password: String,
}

impl fmt::Debug for TestServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestServer")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("superuser", &self.superuser)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// One disposable, empty database. It is dropped with `WITH (FORCE)` when it goes out of scope.
#[derive(Debug)]
pub struct TestDatabase {
    server: TestServer,
    name: String,
}

impl TestServer {
    /// Read the cluster values written by `pg-test`.
    ///
    /// Returns `None` only when none of the three variables is set; the caller then skips the
    /// test. When any of them is set but the configuration is invalid, this panics, so the test
    /// fails instead of passing without a database. The panic message never contains a value.
    ///
    /// # Panics
    ///
    /// Panics when a variable is set and the configuration is incomplete or invalid.
    pub fn from_env() -> Option<Self> {
        let port = env::var_os(PORT_VAR);
        let superuser = env::var_os(USER_VAR);
        let password = env::var_os(PASSWORD_VAR);
        if port.is_none() && superuser.is_none() && password.is_none() {
            return None;
        }
        // A value that is not valid UTF-8 is invalid, so it is treated as empty.
        let port = port.as_deref().map(|value| value.to_str().unwrap_or(""));
        let superuser = superuser
            .as_deref()
            .map(|value| value.to_str().unwrap_or(""));
        let password = password
            .as_deref()
            .map(|value| value.to_str().unwrap_or(""));
        match server_from_values(port, superuser, password) {
            Ok(server) => Some(server),
            Err(reason) => panic!(
                "PostgreSQL test configuration is set but invalid ({reason}); the test fails instead of skipping"
            ),
        }
    }

    /// Create a new, empty UTF-8 database from `template0` and return a handle to it.
    ///
    /// # Panics
    ///
    /// Panics when the cluster cannot be reached or refuses the database creation. The message
    /// never contains the connection values.
    pub async fn fresh_database(&self) -> TestDatabase {
        let name = new_database_name();
        let admin = self.connect(ADMIN_DATABASE).await;
        let statement = format!("CREATE DATABASE \"{name}\" TEMPLATE template0 ENCODING 'UTF8'");
        if admin.batch_execute(&statement).await.is_err() {
            panic!("could not create the disposable test database");
        }
        TestDatabase {
            server: self.clone(),
            name,
        }
    }

    async fn connect(&self, database: &str) -> Client {
        match self.try_connect(database).await {
            Ok(client) => client,
            Err(_) => panic!("could not connect to the disposable PostgreSQL test server"),
        }
    }

    async fn try_connect(&self, database: &str) -> Result<Client, &'static str> {
        let mut config = Config::new();
        config
            .host(self.host)
            .port(self.port)
            .user(&self.superuser)
            .password(&self.password)
            .dbname(database)
            .connect_timeout(CONNECT_TIMEOUT);
        // The timeout bounds the whole connection; a wedged server is reported as an error.
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, config.connect(NoTls)).await;
        let (client, connection) = match connected {
            Ok(Ok(parts)) => parts,
            Ok(Err(_)) => return Err("could not connect to the disposable PostgreSQL test server"),
            Err(_elapsed) => {
                return Err("connecting to the disposable PostgreSQL test server timed out");
            }
        };
        tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(client)
    }
}

impl TestDatabase {
    /// The generated name, `jobniac_test_` followed by 12 lowercase hex digits.
    pub fn database_name(&self) -> &str {
        &self.name
    }

    /// Environment variables that point the `JOBNIAC_DB_*` configuration at this database.
    pub fn environment(&self) -> Environment {
        environment_for(&self.server, &self.name)
    }

    /// Open a superuser connection to this database. The connection task runs on the current
    /// tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics when the connection cannot be opened.
    pub async fn admin_client(&self) -> Client {
        self.server.connect(&self.name).await
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        // The drop runs on a helper thread with its own runtime, so it works from any context
        // and never panics. The wait is bounded: a wedged server is reported as a leaked database
        // instead of blocking the test run.
        let server = self.server.clone();
        let name = self.name.clone();
        let (sender, receiver) = mpsc::channel();
        let cleanup = std::thread::Builder::new()
            .name("jobniac-testkit-drop".to_owned())
            .spawn(move || {
                let _ = sender.send(drop_database(&server, &name));
            });
        let dropped = match cleanup {
            Ok(_) => matches!(receiver.recv_timeout(CLEANUP_TIMEOUT), Ok(Ok(()))),
            Err(_) => false,
        };
        if !dropped {
            eprintln!(
                "jobniac-testkit: could not drop disposable database {}",
                self.name
            );
        }
    }
}

fn drop_database(server: &TestServer, name: &str) -> Result<(), &'static str> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "could not start the cleanup runtime")?;
    let result: Result<(), &'static str> = runtime.block_on(async {
        let client = server.try_connect(ADMIN_DATABASE).await?;
        let statement = format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)");
        client
            .batch_execute(&statement)
            .await
            .map_err(|_| "could not drop the database")
    });
    result
}

fn server_from_values(
    port: Option<&str>,
    superuser: Option<&str>,
    password: Option<&str>,
) -> Result<TestServer, &'static str> {
    let port = port.ok_or("JOBNIAC_TEST_PG_PORT is unset")?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or("JOBNIAC_TEST_PG_PORT is not a valid port")?;
    let superuser = superuser
        .filter(|value| !value.is_empty())
        .ok_or("JOBNIAC_TEST_PG_USER is unset")?;
    let password = password
        .filter(|value| !value.is_empty())
        .ok_or("JOBNIAC_TEST_PG_PASSWORD is unset")?;
    Ok(TestServer {
        host: HOST,
        port,
        superuser: superuser.to_owned(),
        password: password.to_owned(),
    })
}

fn environment_for(server: &TestServer, database: &str) -> Environment {
    Environment::from_pairs([
        ("JOBNIAC_DB_HOST", server.host.to_owned()),
        ("JOBNIAC_DB_PORT", server.port.to_string()),
        ("JOBNIAC_DB_NAME", database.to_owned()),
        ("JOBNIAC_DB_USER", server.superuser.clone()),
        ("JOBNIAC_DB_PASSWORD", server.password.clone()),
    ])
}

fn new_database_name() -> String {
    format!("{DATABASE_PREFIX}{}", random_hex(6))
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    if getrandom::fill(&mut buffer).is_err() {
        panic!("the operating system random source is unavailable");
    }
    hex::encode(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_server() -> TestServer {
        TestServer {
            host: HOST,
            port: 54_321,
            superuser: "jobniac_test_admin".to_owned(),
            password: "synthetic-password".to_owned(),
        }
    }

    #[test]
    fn generated_names_are_prefixed_lowercase_hex() {
        let name = new_database_name();
        let suffix = name
            .strip_prefix("jobniac_test_")
            .expect("generated name has the test prefix");
        assert_eq!(suffix.len(), 12);
        assert!(
            suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_ne!(name, new_database_name());
    }

    #[test]
    fn environment_maps_the_cluster_and_database() {
        let environment = environment_for(&sample_server(), "jobniac_test_example");
        assert_eq!(environment.get("JOBNIAC_DB_HOST"), Some("127.0.0.1"));
        assert_eq!(environment.get("JOBNIAC_DB_PORT"), Some("54321"));
        assert_eq!(
            environment.get("JOBNIAC_DB_NAME"),
            Some("jobniac_test_example")
        );
        assert_eq!(
            environment.get("JOBNIAC_DB_USER"),
            Some("jobniac_test_admin")
        );
        assert_eq!(
            environment.get("JOBNIAC_DB_PASSWORD"),
            Some("synthetic-password")
        );
    }

    #[test]
    fn server_values_are_all_required() {
        let server = server_from_values(Some("5432"), Some("admin"), Some("secret"))
            .expect("complete values are accepted");
        assert_eq!(server.port, 5432);
        assert_eq!(server.superuser, "admin");

        assert_eq!(
            server_from_values(None, Some("admin"), Some("secret")).err(),
            Some("JOBNIAC_TEST_PG_PORT is unset")
        );
        assert_eq!(
            server_from_values(Some("0"), Some("admin"), Some("secret")).err(),
            Some("JOBNIAC_TEST_PG_PORT is not a valid port")
        );
        assert_eq!(
            server_from_values(Some("70000"), Some("admin"), Some("secret")).err(),
            Some("JOBNIAC_TEST_PG_PORT is not a valid port")
        );
        assert_eq!(
            server_from_values(Some("5432"), None, Some("secret")).err(),
            Some("JOBNIAC_TEST_PG_USER is unset")
        );
        assert_eq!(
            server_from_values(Some("5432"), Some("admin"), Some("")).err(),
            Some("JOBNIAC_TEST_PG_PASSWORD is unset")
        );
    }

    #[test]
    fn debug_output_hides_the_password() {
        let rendered = format!("{:?}", sample_server());
        assert!(!rendered.contains("synthetic-password"));
        assert!(rendered.contains("<redacted>"));
    }

    /// End-to-end smoke test: needs the cluster that `pg-test` owns (see the crate README).
    #[tokio::test]
    #[ignore = "needs the cluster owned by pg-test (see crates/testkit/README.md)"]
    async fn fresh_database_is_created_empty_and_dropped() {
        let Some(server) = TestServer::from_env() else {
            return;
        };
        let database = server.fresh_database().await;
        let name = database.database_name().to_owned();
        assert!(name.starts_with(DATABASE_PREFIX));
        assert_eq!(name.len(), DATABASE_PREFIX.len() + 12);

        let environment = database.environment();
        assert_eq!(environment.get("JOBNIAC_DB_NAME"), Some(name.as_str()));

        {
            let client = database.admin_client().await;
            let row = client
                .query_one(
                    "SELECT current_setting('server_version_num'), \
                     current_setting('server_encoding'), \
                     (SELECT count(*) FROM information_schema.tables \
                      WHERE table_schema NOT IN ('pg_catalog', 'information_schema'))",
                    &[],
                )
                .await
                .expect("the disposable database answers queries");
            let version: String = row.get(0);
            let encoding: String = row.get(1);
            let user_tables: i64 = row.get(2);
            let major = version
                .parse::<u32>()
                .expect("server_version_num is numeric")
                / 10_000;
            assert_eq!(major, 18);
            assert_eq!(encoding, "UTF8");
            assert_eq!(user_tables, 0);
        }

        drop(database);

        let client = server.connect(ADMIN_DATABASE).await;
        let row = client
            .query_one(
                "SELECT count(*) FROM pg_database WHERE datname = $1",
                &[&name],
            )
            .await
            .expect("pg_database is readable");
        let remaining: i64 = row.get(0);
        assert_eq!(remaining, 0, "the fresh database was dropped");
    }
}
