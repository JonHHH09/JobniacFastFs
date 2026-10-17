//! SQLSTATE classification of `tokio-postgres` errors.
//!
//! Maps database failures to caller-safe [`AppError`]s. Message text from the server is never
//! carried into an error, because it can contain row values.

use jobniac_core::errors::AppError;

use crate::postgres::unit_of_work::{PgError, is_operational};

/// `serialization_failure`.
pub const SERIALIZATION_FAILURE: &str = "40001";
/// `deadlock_detected`.
pub const DEADLOCK_DETECTED: &str = "40P01";
/// `lock_not_available`.
pub const LOCK_NOT_AVAILABLE: &str = "55P03";
/// `query_canceled`, raised by `statement_timeout` and cancellation.
pub const QUERY_CANCELED: &str = "57014";
/// `read_only_sql_transaction`.
pub const READ_ONLY_SQL_TRANSACTION: &str = "25006";

/// The five-character SQLSTATE of a database error; `None` for I/O, connection and client errors.
pub fn code(error: &tokio_postgres::Error) -> Option<&str> {
    error.as_db_error().map(|db| db.code().code())
}

/// Whether a SQLSTATE of this code is an operational failure.
///
/// The prefix list is that of `unit_of_work`'s `is_operational`, which this delegates to: classes
/// `08`, `27`, `28`, `2F`, `38`, `39`, `3B`, `40`, `53`, `54`, `55`, `57`, `58`, and the two specific
/// prefixes `F0` (configuration file error) and `HV` (foreign-data wrapper error). `H0000` is a base
/// driver error and is not operational. `22`, `23`, `42` and `P` are not operational either.
///
/// The predicate matches on the prefix alone and does not decide the outcome. [`classify`] checks
/// 57014 and 55P03 before the prefix list, so they become [`PgError::QueryCanceled`] and
/// [`PgError::LockNotAvailable`] although their classes are operational. A caller that needs the
/// specific outcome must check those codes first (`health.rs` does so for 57014).
pub fn is_operational_class(code: &str) -> bool {
    is_operational(code)
}

/// Map a failed statement to an application error through the unit of work's classification.
///
/// This is [`PgError::from`] followed by `AppError::from`, so it cannot disagree with the unit of
/// work. A database error is classified by [`classify`]: SQLSTATE class 23 becomes
/// [`AppError::PersistenceConstraint`] with the SQLSTATE and the constraint name, the operational
/// classes become "Local database is unavailable", and query cancellation (57014), lock timeouts
/// (55P03) and every other database error become the generic [`AppError::Internal`]. An error
/// without a SQLSTATE is unavailable only when the connection is closed or an I/O error caused it;
/// any other client-side error is [`AppError::Internal`].
pub fn map_error(error: tokio_postgres::Error) -> AppError {
    AppError::from(PgError::from(error))
}

/// Whether the statement failed with `serialization_failure` (40001).
pub fn is_serialization_failure(error: &tokio_postgres::Error) -> bool {
    code(error) == Some(SERIALIZATION_FAILURE)
}

/// Whether the statement was chosen as a deadlock victim (40P01).
pub fn is_deadlock(error: &tokio_postgres::Error) -> bool {
    code(error) == Some(DEADLOCK_DETECTED)
}

/// Whether a lock could not be acquired within `lock_timeout` (55P03).
pub fn is_lock_not_available(error: &tokio_postgres::Error) -> bool {
    code(error) == Some(LOCK_NOT_AVAILABLE)
}

/// Whether the statement was canceled, typically by `statement_timeout` (57014).
pub fn is_query_canceled(error: &tokio_postgres::Error) -> bool {
    code(error) == Some(QUERY_CANCELED)
}

/// Whether a write was attempted in a read-only transaction (25006).
pub fn is_read_only_transaction(error: &tokio_postgres::Error) -> bool {
    code(error) == Some(READ_ONLY_SQL_TRANSACTION)
}

#[cfg(test)]
mod tests {
    use jobniac_core::errors::ApplicationError;

    use super::*;
    use crate::postgres::unit_of_work::classify;

    #[test]
    fn operational_classes_match_the_unit_of_work() {
        for code in [
            "08006", "08P01", "27000", "28P01", "2F005", "38000", "39P01", "3B001", "40001",
            "40P01", "53300", "54000", "55006", "55P03", "57P01", "57P03", "57014", "58030",
            "F0000", "HV000",
        ] {
            assert!(is_operational_class(code), "{code} is operational");
        }
        for code in [
            "22012", "23505", "24000", "25006", "3D000", "42P01", "44000", "P0001", "XX000",
            "H0000", "F1000",
        ] {
            assert!(!is_operational_class(code), "{code} is not operational");
        }
    }

    /// A client-side error without a SQLSTATE is internal unless the connection closed or an I/O
    /// error caused it, as the unit of work decides. A timeout is neither, so it maps to internal.
    /// `__private_api_timeout` is the only public way to build a driver error without a server, and
    /// it is used here in a test only.
    #[test]
    fn client_errors_without_a_sqlstate_are_internal() {
        let timeout = tokio_postgres::Error::__private_api_timeout();
        assert!(code(&timeout).is_none());
        assert_eq!(map_error(timeout), AppError::Internal);
    }

    /// One row per SQLSTATE of the unit-of-work mapping.
    #[test]
    fn database_errors_map_to_the_planned_categories() {
        let unavailable: AppError = ApplicationError::database_unavailable().into();
        let cases: [(&str, AppError); 11] = [
            (
                "23505",
                AppError::PersistenceConstraint {
                    sqlstate: Some("23505".to_owned()),
                    constraint: Some("uq".to_owned()),
                },
            ),
            ("22012", AppError::Internal),
            ("42P01", AppError::Internal),
            ("25006", AppError::Internal),
            ("P0001", AppError::Internal),
            ("57014", AppError::Internal),
            ("55P03", AppError::Internal),
            ("08006", unavailable.clone()),
            ("40001", unavailable.clone()),
            ("40P01", unavailable.clone()),
            ("53300", unavailable.clone()),
        ];
        for (code, expected) in cases {
            let constraint = (code == "23505").then_some("uq");
            assert_eq!(
                AppError::from(classify(code, constraint)),
                expected,
                "{code}"
            );
        }
    }
}
