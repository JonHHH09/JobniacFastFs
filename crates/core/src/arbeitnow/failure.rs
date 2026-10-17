//! Source-shaped failures of the fixed read-only Arbeitnow board.
//!
//! Only bounded scalar classification crosses the provider port; the type carries no upstream text.
//! The closed enum makes the constraint matrix unrepresentable when violated: transport and
//! rate-limited failures are always retryable, `http_status` may be either, every other category is
//! never retryable, and only rate-limited failures carry a bounded retry-after.

use crate::errors::{ApplicationError, ErrorCode};

/// The only message ever shown for a board failure.
pub const SAFE_MESSAGE: &str = "Arbeitnow job board is temporarily unavailable";

/// Largest accepted `Retry-After`, in seconds (one day).
pub const MAX_RETRY_AFTER_SECONDS: u32 = 86_400;

/// A retry-after value in `0..=86400` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RetryAfterSeconds(u32);

impl RetryAfterSeconds {
    /// `Some` when `seconds` is within `0..=86400`.
    pub const fn new(seconds: u32) -> Option<Self> {
        if seconds <= MAX_RETRY_AFTER_SECONDS {
            Some(Self(seconds))
        } else {
            None
        }
    }

    /// The number of seconds.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// The closed set of failure categories with their stable names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardCategory {
    Transport,
    HttpStatus,
    RateLimited,
    ContentType,
    BodySize,
    Json,
    Schema,
    Pagination,
}

impl BoardCategory {
    /// The wire name (`failureCategory`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::HttpStatus => "http_status",
            Self::RateLimited => "rate_limited",
            Self::ContentType => "content_type",
            Self::BodySize => "body_size",
            Self::Json => "json",
            Self::Schema => "schema",
            Self::Pagination => "pagination",
        }
    }
}

/// A classified board failure. Carries no upstream text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardFailure {
    /// Connection, TLS, timeout or body-read failure (always retryable).
    Transport,
    /// An unexpected HTTP status.
    HttpStatus { retryable: bool },
    /// HTTP 429 (always retryable), with an optional bounded `Retry-After`.
    RateLimited {
        retry_after: Option<RetryAfterSeconds>,
    },
    /// Response content type is not JSON.
    ContentType,
    /// Response body exceeds the size bound.
    BodySize,
    /// Response body is not valid JSON.
    Json,
    /// Response JSON does not match the expected schema.
    Schema,
    /// Pagination links or counters are invalid.
    Pagination,
}

impl BoardFailure {
    /// The failure category.
    pub const fn category(&self) -> BoardCategory {
        match self {
            Self::Transport => BoardCategory::Transport,
            Self::HttpStatus { .. } => BoardCategory::HttpStatus,
            Self::RateLimited { .. } => BoardCategory::RateLimited,
            Self::ContentType => BoardCategory::ContentType,
            Self::BodySize => BoardCategory::BodySize,
            Self::Json => BoardCategory::Json,
            Self::Schema => BoardCategory::Schema,
            Self::Pagination => BoardCategory::Pagination,
        }
    }

    /// Whether the caller may retry.
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Transport | Self::RateLimited { .. } => true,
            Self::HttpStatus { retryable } => *retryable,
            _ => false,
        }
    }

    /// The bounded retry-after, only for rate-limited failures.
    pub const fn retry_after_seconds(&self) -> Option<u32> {
        match self {
            Self::RateLimited {
                retry_after: Some(seconds),
            } => Some(seconds.get()),
            _ => None,
        }
    }

    /// The application error code.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::RateLimited { .. } => ErrorCode::UpstreamRateLimited,
            Self::Transport | Self::HttpStatus { .. } => ErrorCode::UpstreamUnavailable,
            _ => ErrorCode::UpstreamInvalidResponse,
        }
    }

    /// The constant caller-safe message.
    pub const fn safe_message(&self) -> &'static str {
        SAFE_MESSAGE
    }

    /// The wire details: provider, category, retryable and an optional retry-after.
    pub fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("provider".to_owned(), "arbeitnow".to_owned()),
            (
                "failureCategory".to_owned(),
                self.category().name().to_owned(),
            ),
            (
                "retryable".to_owned(),
                if self.retryable() { "true" } else { "false" }.to_owned(),
            ),
        ];
        if let Some(seconds) = self.retry_after_seconds() {
            details.push(("retryAfterSeconds".to_owned(), seconds.to_string()));
        }
        details
    }

    /// The caller-safe application error for this failure.
    pub fn to_application_error(&self) -> ApplicationError {
        ApplicationError::new(self.code(), Some(SAFE_MESSAGE), self.details())
    }
}

impl std::fmt::Display for BoardFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(SAFE_MESSAGE)
    }
}

impl std::error::Error for BoardFailure {}
