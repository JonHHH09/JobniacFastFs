//! Validated, loopback-only PostgreSQL connection settings.
//!
//! Only a complete, strictly validated `JOBNIAC_DB_*` set yields a configuration. Anything else
//! is `None`, so callers can never fall back to libpq defaults, a DSN or a primary database.
//!
//! `tokio-postgres` reads no `PG*` environment variables and no service file. Every connection
//! setting is set explicitly by [`DatabaseConfig::connect_config`]. The pool refuses every
//! connection when `PGSERVICE` is set (a libpq service file could override the validated target),
//! so the variable is recorded here and the pool refuses to connect while it is set.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use jobniac_core::env::Environment;
use secrecy::{ExposeSecret, SecretString};

const HOST_KEY: &str = "JOBNIAC_DB_HOST";
const PORT_KEY: &str = "JOBNIAC_DB_PORT";
const NAME_KEY: &str = "JOBNIAC_DB_NAME";
const USER_KEY: &str = "JOBNIAC_DB_USER";
const PASSWORD_KEY: &str = "JOBNIAC_DB_PASSWORD";
/// The libpq service variable. Every database connection is refused while it is non-empty.
const LIBPQ_SERVICE_KEY: &str = "PGSERVICE";

const MAX_PORT_TEXT_LEN: usize = 5;
const MAX_IDENTIFIER_LEN: usize = 63;
const MAX_PASSWORD_CHARS: usize = 1024;

/// Validated connection parameters for the local PostgreSQL database.
///
/// The password is held as a [`SecretString`] and is never printed by `Debug`.
#[derive(Clone)]
pub struct DatabaseConfig {
    host: String,
    addr: IpAddr,
    port: u16,
    database: String,
    user: String,
    password: SecretString,
    libpq_service_set: bool,
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl DatabaseConfig {
    /// The validated loopback literal, `127.0.0.1` or `::1`.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The validated TCP port (1 to 65535).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The validated database name.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// The validated role name.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Whether the `PGSERVICE` variable was set and non-empty when the configuration was loaded.
    /// The pool refuses every connection then.
    pub fn libpq_service_set(&self) -> bool {
        self.libpq_service_set
    }

    /// Build the complete connection settings for one session.
    ///
    /// `hostaddr` repeats the validated literal, so no host lookup happens. `options` and
    /// `application_name` are set only when given. The timeout is applied as given.
    pub(crate) fn connect_config(
        &self,
        connect_timeout: Duration,
        options: &str,
        application_name: Option<&str>,
    ) -> tokio_postgres::Config {
        let mut config = tokio_postgres::Config::new();
        config
            .host(&self.host)
            .hostaddr(self.addr)
            .port(self.port)
            .dbname(&self.database)
            .user(&self.user)
            .password(self.password.expose_secret())
            .connect_timeout(connect_timeout)
            .options(options);
        if let Some(name) = application_name {
            config.application_name(name);
        }
        config
    }
}

/// Load only a complete, strictly validated `JOBNIAC_DB_*` configuration.
///
/// Missing, empty or malformed values return `None`. The rules: all five variables present and
/// non-empty; host exactly `127.0.0.1` or `::1`; port ASCII decimal of at most five characters in
/// 1..=65535; database and user matching `^[A-Za-z_][A-Za-z0-9_]{0,62}$`; password of at most 1024
/// code points with no code point below U+0020.
pub fn load_database_config(env: &Environment) -> Option<DatabaseConfig> {
    let required = |key: &str| env.get(key).filter(|value| !value.is_empty());
    let host = required(HOST_KEY)?;
    let port_text = required(PORT_KEY)?;
    let database = required(NAME_KEY)?;
    let user = required(USER_KEY)?;
    let password = required(PASSWORD_KEY)?;

    let addr = match host {
        "127.0.0.1" | "::1" => host.parse::<IpAddr>().ok()?,
        _ => return None,
    };
    let port = parse_port(port_text)?;
    if !is_identifier(database) || !is_identifier(user) {
        return None;
    }
    if password.chars().count() > MAX_PASSWORD_CHARS || password.chars().any(|c| c < ' ') {
        return None;
    }

    Some(DatabaseConfig {
        host: host.to_owned(),
        addr,
        port,
        database: database.to_owned(),
        user: user.to_owned(),
        password: SecretString::from(password.to_owned()),
        libpq_service_set: env
            .get(LIBPQ_SERVICE_KEY)
            .is_some_and(|value| !value.is_empty()),
    })
}

/// ASCII decimal digits only, at most five of them, value 1 to 65535.
fn parse_port(text: &str) -> Option<u16> {
    if text.len() > MAX_PORT_TEXT_LEN || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = text.parse().ok()?;
    (port >= 1).then_some(port)
}

/// ASCII `^[A-Za-z_][A-Za-z0-9_]{0,62}$`, matched over the whole string.
fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_IDENTIFIER_LEN
        && text.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic() || byte == b'_'
            } else {
                byte.is_ascii_alphanumeric() || byte == b'_'
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const KEYS: [&str; 5] = [HOST_KEY, PORT_KEY, NAME_KEY, USER_KEY, PASSWORD_KEY];

    /// The valid fixture with the given variables replaced.
    fn env_with(overrides: &[(&str, &str)]) -> Environment {
        let mut values: BTreeMap<String, String> = [
            (HOST_KEY, "127.0.0.1"),
            (PORT_KEY, "5432"),
            (NAME_KEY, "jobniac_test"),
            (USER_KEY, "app_user"),
            (PASSWORD_KEY, "s3cret!"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        for (key, value) in overrides {
            values.insert((*key).to_owned(), (*value).to_owned());
        }
        Environment::from_pairs(values)
    }

    fn accepted(overrides: &[(&str, &str)]) -> Option<DatabaseConfig> {
        load_database_config(&env_with(overrides))
    }

    #[test]
    fn valid_configurations_are_accepted() {
        let config = accepted(&[]).expect_config();
        assert_eq!(config.host(), "127.0.0.1");
        assert_eq!(config.port(), 5432);
        assert_eq!(config.database(), "jobniac_test");
        assert_eq!(config.user(), "app_user");

        let ipv6 = accepted(&[(HOST_KEY, "::1"), (PORT_KEY, "00080")]).expect_config();
        assert_eq!(ipv6.host(), "::1");
        assert_eq!(ipv6.port(), 80);
        assert!(accepted(&[(PORT_KEY, "65535")]).is_some());
        assert!(accepted(&[(NAME_KEY, "_a1")]).is_some());
        let longest = "a".repeat(63);
        assert!(accepted(&[(USER_KEY, longest.as_str())]).is_some());
    }

    #[test]
    fn each_validation_rule_rejects_its_violation() {
        let long_name = "a".repeat(64);
        let long_password = "x".repeat(1025);
        let cases: Vec<(&str, String)> = vec![
            (HOST_KEY, "localhost".to_owned()),
            (HOST_KEY, "127.0.0.2".to_owned()),
            (HOST_KEY, "0:0:0:0:0:0:0:1".to_owned()),
            (PORT_KEY, "0".to_owned()),
            (PORT_KEY, "65536".to_owned()),
            (PORT_KEY, "123456".to_owned()),
            (PORT_KEY, "+5432".to_owned()),
            (PORT_KEY, "5432 ".to_owned()),
            (PORT_KEY, "\u{FF15}\u{FF14}\u{FF13}\u{FF12}".to_owned()),
            (PORT_KEY, "5.4".to_owned()),
            (NAME_KEY, "1abc".to_owned()),
            (NAME_KEY, "a-b".to_owned()),
            (NAME_KEY, long_name),
            (USER_KEY, "app\n".to_owned()),
            (USER_KEY, "\u{00E9}tat".to_owned()),
            (PASSWORD_KEY, "bad\u{0001}pass".to_owned()),
            (PASSWORD_KEY, long_password),
        ];
        for (key, value) in cases {
            assert!(
                accepted(&[(key, value.as_str())]).is_none(),
                "{key} = {value:?} should be rejected"
            );
        }
    }

    #[test]
    fn missing_empty_and_invalid_values_are_rejected() {
        for key in KEYS {
            assert!(
                accepted(&[(key, "")]).is_none(),
                "empty {key} should be rejected"
            );
            let env = env_with(&[]);
            let without: Vec<(String, String)> = KEYS
                .iter()
                .filter(|k| **k != key)
                .filter_map(|k| env.get(k).map(|v| ((*k).to_owned(), v.to_owned())))
                .collect();
            assert!(
                load_database_config(&Environment::from_pairs(without)).is_none(),
                "missing {key} should be rejected"
            );
            assert!(
                load_database_config(&env_with(&[]).with_invalid(key)).is_none(),
                "invalid {key} should be rejected"
            );
        }
    }

    #[test]
    fn password_length_counts_code_points() {
        let accented = "\u{00E9}".repeat(1024);
        assert!(accepted(&[(PASSWORD_KEY, accented.as_str())]).is_some());
        let too_long = "\u{00E9}".repeat(1025);
        assert!(accepted(&[(PASSWORD_KEY, too_long.as_str())]).is_none());
    }

    #[test]
    fn debug_output_never_contains_the_password() {
        let config = accepted(&[]).expect_config();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("s3cret"));
        assert!(rendered.contains("jobniac_test"));
    }

    #[test]
    fn connect_config_pins_the_validated_target() -> Result<(), Box<dyn std::error::Error>> {
        let config = accepted(&[(HOST_KEY, "::1")]).expect_config();
        let pg = config.connect_config(Duration::from_secs(2), "-c lock_timeout=1000", None);
        assert_eq!(pg.get_hostaddrs(), &["::1".parse::<IpAddr>()?]);
        assert_eq!(pg.get_ports(), &[5432_u16]);
        assert_eq!(pg.get_user(), Some("app_user"));
        assert_eq!(pg.get_dbname(), Some("jobniac_test"));
        assert_eq!(pg.get_options(), Some("-c lock_timeout=1000"));
        assert_eq!(pg.get_application_name(), None);
        assert_eq!(pg.get_connect_timeout(), Some(&Duration::from_secs(2)));

        let named = config.connect_config(Duration::from_secs(5), "", Some("jobniac-migrate"));
        assert_eq!(named.get_application_name(), Some("jobniac-migrate"));
        Ok(())
    }

    /// Unwraps a validated fixture; a rejected valid fixture fails the test.
    trait ExpectConfig {
        fn expect_config(self) -> DatabaseConfig;
    }

    impl ExpectConfig for Option<DatabaseConfig> {
        fn expect_config(self) -> DatabaseConfig {
            match self {
                Some(config) => config,
                None => panic!("valid fixture configuration was rejected"),
            }
        }
    }
}
