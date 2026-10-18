//! Classification of SQLite failures.
//!
//! [`SqliteError`] is the counterpart of `PgError`: the same caller-safe classes and the same fixed
//! user-visible messages, with SQLite result codes in place of SQLSTATEs. Message text from the
//! engine is never carried into an error, because it can contain row values. The only text kept is
//! the name of a violated constraint (a schema identifier, never a value).
//!
//! Mapping:
//! * `SQLITE_CONSTRAINT*` becomes [`SqliteError::Constraint`], with the SQLSTATE of the equivalent
//!   PostgreSQL class 23 error (unique 23505, foreign key 23503, not null 23502, check and trigger
//!   23514, anything else 23000), so `AppError::PersistenceConstraint` is unchanged.
//! * `SQLITE_BUSY` and `SQLITE_LOCKED` become [`SqliteError::Busy`]. The unit of work retries them a
//!   bounded number of times first; an exhausted retry is "Local database is unavailable".
//! * I/O, corrupt, cannot-open, full-disk, not-a-database and permission failures become
//!   [`SqliteError::Unavailable`].
//! * Everything else is [`SqliteError::Internal`]: a type mismatch in a repository, and
//!   `SQLITE_READONLY` (a write in a read-only scope, like PostgreSQL's 25006).

use std::fmt;
use std::os::raw::c_int;

use jobniac_core::errors::{AppError, ApplicationError, ErrorCode};
use rusqlite::ffi;

/// Message of [`SqliteError::NotConfigured`] (the same text as the PostgreSQL adapter).
pub const NOT_CONFIGURED_MESSAGE: &str = "Local database is not configured";

/// A classified database failure of the unit of work.
///
/// No variant carries engine message text, so no row value or SQL reaches a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqliteError {
    /// No database file is configured.
    NotConfigured,
    /// Open, pool wait, I/O or other operational failure.
    Unavailable,
    /// `SQLITE_BUSY` or `SQLITE_LOCKED` after the bounded retry. Reported as unavailable.
    Busy,
    /// An integrity-constraint violation (including a trigger that aborts).
    Constraint {
        /// The SQLSTATE of the equivalent PostgreSQL error.
        sqlstate: String,
        /// Name of the violated constraint (or the failing column list), when the engine reports
        /// one.
        constraint: Option<String>,
    },
    /// A write was requested in a read-only scope that a joined scope cannot change.
    ReadOnlyJoin,
    /// A `requires_new` write scope was requested while the enclosing scope already holds the
    /// single writer. A second writer would wait for the first, so it is refused.
    NestedWrite,
    /// Any other database failure, or a client-side bug such as a column type mismatch.
    Internal,
}

impl SqliteError {
    /// A constraint violation raised by a pre-commit check (the replacement of a deferred
    /// PostgreSQL constraint trigger). It maps like a check violation (23514).
    pub fn constraint(name: &str) -> Self {
        Self::Constraint {
            sqlstate: SQLSTATE_CHECK.to_owned(),
            constraint: Some(name.to_owned()),
        }
    }

    /// Whether this is a lock failure that a retry can resolve.
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Busy)
    }
}

impl fmt::Display for SqliteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::NotConfigured => "not configured",
            Self::Unavailable => "unavailable",
            Self::Busy => "busy",
            Self::Constraint { .. } => "constraint violation",
            Self::ReadOnlyJoin => "read-only join",
            Self::NestedWrite => "nested write",
            Self::Internal => "internal",
        };
        write!(f, "database error: {kind}")
    }
}

impl std::error::Error for SqliteError {}

const SQLSTATE_UNIQUE: &str = "23505";
const SQLSTATE_FOREIGN_KEY: &str = "23503";
const SQLSTATE_NOT_NULL: &str = "23502";
const SQLSTATE_CHECK: &str = "23514";
const SQLSTATE_OTHER: &str = "23000";

impl From<rusqlite::Error> for SqliteError {
    fn from(error: rusqlite::Error) -> Self {
        match &error {
            rusqlite::Error::SqliteFailure(failure, message) => {
                classify(failure.extended_code, message.as_deref())
            }
            _ => Self::Internal,
        }
    }
}

/// Classify a failure by its extended result code and the engine's message.
///
/// The message is read only to extract the constraint name of a constraint violation; it is never
/// stored whole.
pub fn classify(extended_code: c_int, message: Option<&str>) -> SqliteError {
    match extended_code & 0xff {
        ffi::SQLITE_CONSTRAINT => SqliteError::Constraint {
            sqlstate: constraint_sqlstate(extended_code, message).to_owned(),
            constraint: message.and_then(constraint_name),
        },
        ffi::SQLITE_BUSY | ffi::SQLITE_LOCKED => SqliteError::Busy,
        ffi::SQLITE_IOERR
        | ffi::SQLITE_CORRUPT
        | ffi::SQLITE_CANTOPEN
        | ffi::SQLITE_FULL
        | ffi::SQLITE_NOTADB
        | ffi::SQLITE_PERM
        | ffi::SQLITE_PROTOCOL
        | ffi::SQLITE_NOMEM
        | ffi::SQLITE_NOLFS => SqliteError::Unavailable,
        _ => SqliteError::Internal,
    }
}

/// The engine's message of every foreign-key violation. A `RESTRICT` action is reported with the
/// trigger extended code, so the message is what identifies it.
const FOREIGN_KEY_MESSAGE: &str = "FOREIGN KEY constraint failed";

/// The SQLSTATE of the PostgreSQL error that the constraint violation corresponds to.
fn constraint_sqlstate(extended_code: c_int, message: Option<&str>) -> &'static str {
    if message == Some(FOREIGN_KEY_MESSAGE) {
        return SQLSTATE_FOREIGN_KEY;
    }
    match extended_code {
        ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY => SQLSTATE_UNIQUE,
        ffi::SQLITE_CONSTRAINT_FOREIGNKEY => SQLSTATE_FOREIGN_KEY,
        ffi::SQLITE_CONSTRAINT_NOTNULL => SQLSTATE_NOT_NULL,
        ffi::SQLITE_CONSTRAINT_CHECK | ffi::SQLITE_CONSTRAINT_TRIGGER => SQLSTATE_CHECK,
        _ => SQLSTATE_OTHER,
    }
}

/// The schema identifier named by an engine constraint message, if it has one: the CHECK name, the
/// `table.column` list of a UNIQUE, PRIMARY KEY or NOT NULL failure, or the index name of a unique
/// expression index. A trigger message is a fixed text of the schema and carries no identifier.
fn constraint_name(message: &str) -> Option<String> {
    const PREFIXES: [&str; 4] = [
        "CHECK constraint failed: ",
        "UNIQUE constraint failed: ",
        "PRIMARY KEY constraint failed: ",
        "NOT NULL constraint failed: ",
    ];
    let rest = PREFIXES
        .iter()
        .find_map(|prefix| message.strip_prefix(prefix))?;
    // An expression index is reported as `index 'name'`.
    let identifier = match rest
        .strip_prefix("index '")
        .and_then(|name| name.strip_suffix('\''))
    {
        Some(index) => index,
        None => rest,
    };
    Some(identifier.to_owned())
}

impl From<SqliteError> for AppError {
    fn from(error: SqliteError) -> Self {
        match error {
            SqliteError::NotConfigured => {
                ApplicationError::with_message(ErrorCode::InternalError, NOT_CONFIGURED_MESSAGE)
                    .into()
            }
            SqliteError::Unavailable | SqliteError::Busy => {
                ApplicationError::database_unavailable().into()
            }
            SqliteError::Constraint {
                sqlstate,
                constraint,
            } => AppError::PersistenceConstraint {
                sqlstate: Some(sqlstate),
                constraint,
            },
            // The envelope is the generic internal error.
            SqliteError::ReadOnlyJoin | SqliteError::NestedWrite | SqliteError::Internal => {
                AppError::Internal
            }
        }
    }
}

/// Error policy of the profile-details path.
///
/// Integrity failures are a persistence constraint. Every other database failure is "Local database
/// is unavailable". The not-configured error keeps its own message. Profile and deletion
/// repositories map their errors through this function; the generic path keeps
/// [`From<SqliteError> for AppError`].
pub fn details_app_error(error: SqliteError) -> AppError {
    match error {
        SqliteError::NotConfigured => AppError::from(SqliteError::NotConfigured),
        constraint @ SqliteError::Constraint { .. } => AppError::from(constraint),
        _ => ApplicationError::database_unavailable().into(),
    }
}

#[cfg(test)]
mod tests {
    use jobniac_core::errors::error_envelope;

    use super::*;

    /// One row per extended result code of the mapping table in the module documentation.
    #[test]
    fn result_codes_map_to_the_planned_categories() {
        let unavailable = SqliteError::Unavailable;
        let cases: [(c_int, SqliteError); 15] = [
            (
                ffi::SQLITE_CONSTRAINT_UNIQUE,
                SqliteError::Constraint {
                    sqlstate: "23505".to_owned(),
                    constraint: None,
                },
            ),
            (
                ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
                SqliteError::Constraint {
                    sqlstate: "23505".to_owned(),
                    constraint: None,
                },
            ),
            (
                ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
                SqliteError::Constraint {
                    sqlstate: "23503".to_owned(),
                    constraint: None,
                },
            ),
            (
                ffi::SQLITE_CONSTRAINT_NOTNULL,
                SqliteError::Constraint {
                    sqlstate: "23502".to_owned(),
                    constraint: None,
                },
            ),
            (
                ffi::SQLITE_CONSTRAINT_CHECK,
                SqliteError::Constraint {
                    sqlstate: "23514".to_owned(),
                    constraint: None,
                },
            ),
            (
                ffi::SQLITE_CONSTRAINT_TRIGGER,
                SqliteError::Constraint {
                    sqlstate: "23514".to_owned(),
                    constraint: None,
                },
            ),
            (
                ffi::SQLITE_CONSTRAINT,
                SqliteError::Constraint {
                    sqlstate: "23000".to_owned(),
                    constraint: None,
                },
            ),
            (ffi::SQLITE_BUSY, SqliteError::Busy),
            (ffi::SQLITE_BUSY_SNAPSHOT, SqliteError::Busy),
            (ffi::SQLITE_LOCKED, SqliteError::Busy),
            (ffi::SQLITE_IOERR_READ, unavailable.clone()),
            (ffi::SQLITE_CORRUPT, unavailable.clone()),
            (ffi::SQLITE_CANTOPEN, unavailable.clone()),
            (ffi::SQLITE_FULL, unavailable),
            (ffi::SQLITE_READONLY, SqliteError::Internal),
        ];
        for (code, expected) in cases {
            assert_eq!(classify(code, None), expected, "code {code}");
        }
        assert_eq!(classify(ffi::SQLITE_ERROR, None), SqliteError::Internal);
        assert_eq!(classify(ffi::SQLITE_SCHEMA, None), SqliteError::Internal);
    }

    #[test]
    fn constraint_names_come_from_the_engine_message_without_values() {
        let named = classify(
            ffi::SQLITE_CONSTRAINT_CHECK,
            Some("CHECK constraint failed: jobs_title_not_blank"),
        );
        assert_eq!(
            named,
            SqliteError::Constraint {
                sqlstate: "23514".to_owned(),
                constraint: Some("jobs_title_not_blank".to_owned()),
            }
        );
        let unique = classify(
            ffi::SQLITE_CONSTRAINT_UNIQUE,
            Some("UNIQUE constraint failed: profile_profiles.email"),
        );
        assert!(matches!(
            unique,
            SqliteError::Constraint { constraint: Some(ref name), .. }
                if name == "profile_profiles.email"
        ));
        let index = classify(
            ffi::SQLITE_CONSTRAINT_UNIQUE,
            Some("UNIQUE constraint failed: index 'idx_profiles_email_canonical_unique'"),
        );
        assert!(matches!(
            index,
            SqliteError::Constraint { constraint: Some(ref name), .. }
                if name == "idx_profiles_email_canonical_unique"
        ));
        // A RESTRICT violation carries the trigger code but is a foreign-key violation.
        assert!(matches!(
            classify(ffi::SQLITE_CONSTRAINT_TRIGGER, Some("FOREIGN KEY constraint failed")),
            SqliteError::Constraint { ref sqlstate, constraint: None } if sqlstate == "23503"
        ));
        // A trigger or foreign-key message names no identifier.
        let trigger = classify(
            ffi::SQLITE_CONSTRAINT_TRIGGER,
            Some("match history is immutable"),
        );
        assert!(matches!(
            trigger,
            SqliteError::Constraint {
                constraint: None,
                ..
            }
        ));
    }

    /// Driver errors that are not engine failures are repository bugs.
    #[test]
    fn client_side_driver_errors_are_internal() {
        assert_eq!(
            SqliteError::from(rusqlite::Error::QueryReturnedNoRows),
            SqliteError::Internal
        );
        assert_eq!(
            SqliteError::from(rusqlite::Error::InvalidColumnIndex(3)),
            SqliteError::Internal
        );
    }

    #[test]
    fn app_error_conversion_keeps_the_envelopes() {
        let not_configured: AppError = SqliteError::NotConfigured.into();
        let envelope = error_envelope(&not_configured);
        assert_eq!(envelope.code, ErrorCode::InternalError);
        assert_eq!(envelope.message, NOT_CONFIGURED_MESSAGE);
        assert!(envelope.details.is_empty());

        for error in [SqliteError::Unavailable, SqliteError::Busy] {
            let envelope = error_envelope(&AppError::from(error));
            assert_eq!(envelope.message, "Local database is unavailable");
        }

        let constraint: AppError = SqliteError::Constraint {
            sqlstate: "23505".to_owned(),
            constraint: Some("uq".to_owned()),
        }
        .into();
        assert!(matches!(
            constraint,
            AppError::PersistenceConstraint {
                sqlstate: Some(ref state),
                constraint: Some(ref name),
            } if state == "23505" && name == "uq"
        ));
        for error in [
            SqliteError::ReadOnlyJoin,
            SqliteError::NestedWrite,
            SqliteError::Internal,
        ] {
            assert_eq!(AppError::from(error), AppError::Internal);
        }
    }

    /// The pre-commit check error has the class of the PostgreSQL deferred trigger (23514).
    #[test]
    fn pre_commit_check_errors_are_check_violations() {
        assert!(matches!(
            AppError::from(SqliteError::constraint("job_source_provenance")),
            AppError::PersistenceConstraint {
                sqlstate: Some(ref state),
                constraint: Some(ref name),
            } if state == "23514" && name == "job_source_provenance"
        ));
    }

    /// The profile-details policy turns every failure except a constraint into "unavailable".
    #[test]
    fn details_policy_maps_non_constraint_failures_to_unavailable() {
        let unavailable = error_envelope(&AppError::from(ApplicationError::database_unavailable()));
        for error in [
            SqliteError::Unavailable,
            SqliteError::Busy,
            SqliteError::ReadOnlyJoin,
            SqliteError::NestedWrite,
            SqliteError::Internal,
        ] {
            let envelope = error_envelope(&details_app_error(error.clone()));
            assert_eq!(envelope, unavailable, "{error:?}");
        }
        assert!(matches!(
            details_app_error(SqliteError::constraint("x")),
            AppError::PersistenceConstraint { .. }
        ));
        assert_eq!(
            error_envelope(&details_app_error(SqliteError::NotConfigured)).message,
            NOT_CONFIGURED_MESSAGE
        );
    }

    #[test]
    fn errors_render_without_values() {
        assert_eq!(
            SqliteError::Constraint {
                sqlstate: "23505".to_owned(),
                constraint: Some("secret_row_value".to_owned()),
            }
            .to_string(),
            "database error: constraint violation"
        );
    }
}
