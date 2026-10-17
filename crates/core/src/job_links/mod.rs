//! Job link ingestion: URL identity, the pre-fetch URL check, HTML extraction and content admission.
//!
//! The network part of the fetch (the owned child, the transport and the connect loop) is not in
//! this module. It is the fetch port that the infrastructure layer implements on top of
//! [`urls::wire_url`], [`html::admit_response_headers`] and [`html::parse_job_html`]. Its failures
//! use [`JobLinkFetchFailure`] only, so no retrieval URL, header or body text reaches an error.

pub mod html;
pub mod service;
pub mod urls;

/// Classified fetch failure Each variant has one fixed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobLinkFetchFailure {
    /// The content could not be fetched or was refused by admission.
    Unavailable,
    /// The fetch was interrupted by the caller.
    Interrupted,
    /// The retrieval URL is not an acceptable absolute http(s) URL.
    InvalidUrl,
    /// The server answered with a redirect; redirects are never followed.
    RedirectNotFollowed,
    /// The host resolves to a non-public address.
    UnsafeAddress,
    /// Only an IP literal is fetched for plain http (address policy must bind to the connection).
    IpLiteralRequired,
}

impl JobLinkFetchFailure {
    /// The upper-case member name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "UNAVAILABLE",
            Self::Interrupted => "INTERRUPTED",
            Self::InvalidUrl => "INVALID_URL",
            Self::RedirectNotFollowed => "REDIRECT_NOT_FOLLOWED",
            Self::UnsafeAddress => "UNSAFE_ADDRESS",
            Self::IpLiteralRequired => "IP_LITERAL_REQUIRED",
        }
    }

    /// The fixed message for the classification.
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "job link could not be fetched",
            Self::Interrupted => "job link fetch was interrupted",
            Self::InvalidUrl => "must be a valid absolute http(s) URL",
            Self::RedirectNotFollowed => "job link redirects are not followed",
            Self::UnsafeAddress => "host resolves to a private or otherwise unsafe address",
            Self::IpLiteralRequired => {
                "job link fetch requires an IP literal so address policy is connection-bound"
            }
        }
    }
}

/// The fetch port the link service calls. The infrastructure layer
/// implements it with the owned child and the transport, on top of [`urls::wire_url`] and the
/// admission rules in [`html`]. Implementations must return only the classifications above.
pub trait JobLinkContentFetcher {
    /// Fetches and extracts one job page for an already validated retrieval URL.
    fn fetch(&self, retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure>;
}

/// Title, description and status of fetched content.
#[derive(Clone, PartialEq, Eq)]
pub struct JobLinkFetchResult {
    /// The extracted title, if any.
    pub title: Option<String>,
    /// The extracted description (at most 20000 UTF-16 units), if any.
    pub description: Option<String>,
    /// The HTTP status code of the response.
    pub http_status: i64,
}

/// Prints only the status: title and description are page text and never appear in debug output.
impl std::fmt::Debug for JobLinkFetchResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobLinkFetchResult")
            .field("http_status", &self.http_status)
            .finish_non_exhaustive()
    }
}
