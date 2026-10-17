//! Application error model of the MCP wire contract.
//!
//! The public envelope is `{"code", "message", "details"}` with lower-case codes.
//! Messages are always caller-safe: they never contain DSNs, SQL, file contents or stack traces.

use std::fmt;

use indexmap::IndexMap;
use serde::Serialize;

/// Message used when a database integrity constraint rejects a write.
pub const PERSISTENCE_CONSTRAINT_MESSAGE: &str = "Request data violates a persistence constraint";

/// Application error codes: wire code plus default safe message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    ValidationError,
    AuthorizationError,
    NotFound,
    Conflict,
    UpstreamRateLimited,
    UpstreamInvalidResponse,
    UpstreamUnavailable,
    InternalError,
}

impl ErrorCode {
    /// The lower-case wire code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::ValidationError => "validation_error",
            Self::AuthorizationError => "authorization_error",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::UpstreamRateLimited => "upstream_rate_limited",
            Self::UpstreamInvalidResponse => "upstream_invalid_response",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::InternalError => "internal_error",
        }
    }

    /// The default caller-safe message for the code.
    pub const fn default_message(self) -> &'static str {
        match self {
            Self::ValidationError => "Request validation failed",
            Self::AuthorizationError => "Request is not authorized",
            Self::NotFound => "Requested resource was not found",
            Self::Conflict => "Resource revision conflict",
            Self::UpstreamRateLimited => "Upstream service rate limit exceeded",
            Self::UpstreamInvalidResponse => "Upstream service returned an invalid response",
            Self::UpstreamUnavailable => "Upstream service is temporarily unavailable",
            Self::InternalError => "Unexpected application error",
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

/// The wire envelope. Field order is part of the contract (`code`, `message`, `details`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorEnvelope {
    pub code: ErrorCode,
    pub message: String,
    pub details: IndexMap<String, String>,
}

/// A classified, caller-safe failure.
///
/// `details` values are strings (a `Map<String, String>` envelope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationError {
    code: ErrorCode,
    message: String,
    details: IndexMap<String, String>,
}

impl ApplicationError {
    /// Build an error; a missing or blank message falls back to the code's default message.
    pub fn new<I, K, V>(code: ErrorCode, message: Option<&str>, details: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let message = match message {
            Some(text) if !is_unicode_blank(text) => text.to_owned(),
            _ => code.default_message().to_owned(),
        };
        Self {
            code,
            message,
            details: details
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// An error with the code's default message and no details.
    pub fn of(code: ErrorCode) -> Self {
        Self::new(code, None, std::iter::empty::<(String, String)>())
    }

    /// An error with a message and no details.
    pub fn with_message(code: ErrorCode, message: &str) -> Self {
        Self::new(code, Some(message), std::iter::empty::<(String, String)>())
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &IndexMap<String, String> {
        &self.details
    }

    pub fn to_envelope(&self) -> ErrorEnvelope {
        ErrorEnvelope {
            code: self.code,
            message: self.message.clone(),
            details: self.details.clone(),
        }
    }

    /// The configured local database is missing, unreachable or not migrated
    /// (default message).
    pub fn database_unavailable() -> Self {
        Self::with_message(ErrorCode::InternalError, "Local database is unavailable")
    }
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApplicationError {}

/// Build the common `validation_error` shape with optional `field`/`reason` details.
pub fn validation_error(
    message: &str,
    field: Option<&str>,
    reason: Option<&str>,
) -> ApplicationError {
    let mut details = IndexMap::new();
    if let Some(field) = field {
        details.insert("field".to_owned(), field.to_owned());
    }
    if let Some(reason) = reason {
        details.insert("reason".to_owned(), reason.to_owned());
    }
    ApplicationError::new(ErrorCode::ValidationError, Some(message), details)
}

/// A `not_found` error with string details.
pub fn not_found<I, K, V>(message: &str, details: I) -> ApplicationError
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    ApplicationError::new(ErrorCode::NotFound, Some(message), details)
}

/// A `conflict` error with string details.
pub fn conflict<I, K, V>(message: &str, details: I) -> ApplicationError
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    ApplicationError::new(ErrorCode::Conflict, Some(message), details)
}

/// Every failure a use case can surface, mapped to the wire envelope by [`error_envelope`].
///
/// The mapping covers application errors, domain value errors (caller-safe validation
/// message), persistence constraint errors (SQLSTATE class 23) and any other failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Application(ApplicationError),
    /// Caller-safe validation failure whose message describes the rule, never the input.
    #[error("{0}")]
    DomainValue(String),
    /// A database integrity-constraint violation; only SQLSTATE and constraint name are kept.
    #[error("{PERSISTENCE_CONSTRAINT_MESSAGE}")]
    PersistenceConstraint {
        sqlstate: Option<String>,
        constraint: Option<String>,
    },
    /// Anything else. The cause is intentionally not carried, so it cannot leak.
    #[error("Unexpected application error")]
    Internal,
}

impl From<ApplicationError> for AppError {
    fn from(error: ApplicationError) -> Self {
        Self::Application(error)
    }
}

/// Maps any application failure to the wire envelope.
pub fn error_envelope(error: &AppError) -> ErrorEnvelope {
    match error {
        AppError::Application(error) => error.to_envelope(),
        AppError::DomainValue(message) => {
            let trimmed = strip_unicode_whitespace(message);
            let message = if trimmed.is_empty() {
                ErrorCode::ValidationError.default_message().to_owned()
            } else {
                trimmed.to_owned()
            };
            ErrorEnvelope {
                code: ErrorCode::ValidationError,
                message,
                details: IndexMap::new(),
            }
        }
        AppError::PersistenceConstraint { .. } => ErrorEnvelope {
            code: ErrorCode::ValidationError,
            message: PERSISTENCE_CONSTRAINT_MESSAGE.to_owned(),
            details: IndexMap::new(),
        },
        AppError::Internal => ErrorEnvelope {
            code: ErrorCode::InternalError,
            message: ErrorCode::InternalError.default_message().to_owned(),
            details: IndexMap::new(),
        },
    }
}

// Unicode whitespace trimming, needed here before `jobniac-text` exists as a
// dependency of this module. These code points count as whitespace.
fn is_unicode_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

fn strip_unicode_whitespace(text: &str) -> &str {
    text.trim_matches(is_unicode_whitespace)
}

fn is_unicode_blank(text: &str) -> bool {
    strip_unicode_whitespace(text).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_message_falls_back_to_default() {
        let error = ApplicationError::new(ErrorCode::NotFound, Some(" \u{3000}\n"), [("a", "b")]);
        assert_eq!(error.message(), "Requested resource was not found");
        assert_eq!(error.details().get("a").map(String::as_str), Some("b"));
    }

    #[test]
    fn envelope_serializes_in_field_order() {
        let error = validation_error("limit must be positive", Some("limit"), None);
        let json = serde_json::to_string(&error.to_envelope()).unwrap_or_default();
        assert_eq!(
            json,
            r#"{"code":"validation_error","message":"limit must be positive","details":{"field":"limit"}}"#
        );
    }

    #[test]
    fn domain_value_is_stripped_and_defaults_when_blank() {
        let envelope = error_envelope(&AppError::DomainValue(
            "  fullName must not be blank \n".into(),
        ));
        assert_eq!(envelope.message, "fullName must not be blank");
        let envelope = error_envelope(&AppError::DomainValue("   ".into()));
        assert_eq!(envelope.message, "Request validation failed");
    }

    #[test]
    fn constraint_and_internal_errors_are_sanitized() {
        let envelope = error_envelope(&AppError::PersistenceConstraint {
            sqlstate: Some("23505".into()),
            constraint: Some("jobs_canonical_fingerprint_unique".into()),
        });
        assert_eq!(envelope.code, ErrorCode::ValidationError);
        assert_eq!(envelope.message, PERSISTENCE_CONSTRAINT_MESSAGE);
        assert!(envelope.details.is_empty());
        let envelope = error_envelope(&AppError::Internal);
        assert_eq!(envelope.code.code(), "internal_error");
        assert_eq!(envelope.message, "Unexpected application error");
    }
}
