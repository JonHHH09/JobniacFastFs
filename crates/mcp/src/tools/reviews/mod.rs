//! The advisory review tools: `submit_match_review`, `get_match_review`, `list_match_reviews`,
//! `list_match_disagreements`, `acknowledge_match_disagreement` and `link_match_disagreement`
//! (the `MatchReviewService` port).
//!
//! Each tool decodes its `request` in the shape of its `TypedDict`, with the enumerations and
//! strict flags checked in place (see [`decode`]), and then calls one port with the decoded request
//! object. A port answers with the service result, as a JSON value, or with
//! an [`AppError`]. The port is supplied by the composition root (the CLI), so this crate does not
//! depend on the PostgreSQL adapter; an absent port means absent tools. The review tools report
//! their errors with the default envelope text, as the matching tools do.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `submit_match_review`.
pub const SUBMIT_MATCH_REVIEW: &str = "submit_match_review";
/// The catalog name of `get_match_review`.
pub const GET_MATCH_REVIEW: &str = "get_match_review";
/// The catalog name of `list_match_reviews`.
pub const LIST_MATCH_REVIEWS: &str = "list_match_reviews";
/// The catalog name of `list_match_disagreements`.
pub const LIST_MATCH_DISAGREEMENTS: &str = "list_match_disagreements";
/// The catalog name of `acknowledge_match_disagreement`.
pub const ACKNOWLEDGE_MATCH_DISAGREEMENT: &str = "acknowledge_match_disagreement";
/// The catalog name of `link_match_disagreement`.
pub const LINK_MATCH_DISAGREEMENT: &str = "link_match_disagreement";

/// The envelope style of every review tool (`match_review_mcp` reuses `match_mcp._call`).
const STYLE: EnvelopeStyle = EnvelopeStyle::Default;

/// The advisory review and disagreement use cases (`MatchReviewService`). Each method takes the
/// decoded `request` object of its tool, with the keys the `TypedDict` declares.
#[async_trait]
pub trait MatchReviewPort: std::fmt::Debug + Send + Sync {
    /// Persist one advisory review, and the deduplicated disagreement it may create.
    async fn submit_match_review(&self, request: &Value) -> Result<Value, AppError>;
    /// One immutable advisory review by its identifier.
    async fn get_match_review(&self, request: &Value) -> Result<Value, AppError>;
    /// One bounded page of advisory reviews.
    async fn list_match_reviews(&self, request: &Value) -> Result<Value, AppError>;
    /// One bounded page of disagreements.
    async fn list_match_disagreements(&self, request: &Value) -> Result<Value, AppError>;
    /// Acknowledge a disagreement, with an optional opaque issue reference.
    async fn acknowledge_match_disagreement(&self, request: &Value) -> Result<Value, AppError>;
    /// Link a disagreement to an opaque issue reference, without contacting its provider.
    async fn link_match_disagreement(&self, request: &Value) -> Result<Value, AppError>;
}

/// `submit_match_review`: persist one advisory review.
#[derive(Debug)]
pub struct SubmitMatchReviewTool {
    port: Arc<dyn MatchReviewPort>,
}

/// `get_match_review`: one advisory review.
#[derive(Debug)]
pub struct GetMatchReviewTool {
    port: Arc<dyn MatchReviewPort>,
}

/// `list_match_reviews`: one bounded page of advisory reviews.
#[derive(Debug)]
pub struct ListMatchReviewsTool {
    port: Arc<dyn MatchReviewPort>,
}

/// `list_match_disagreements`: one bounded page of disagreements.
#[derive(Debug)]
pub struct ListMatchDisagreementsTool {
    port: Arc<dyn MatchReviewPort>,
}

/// `acknowledge_match_disagreement`: acknowledge one disagreement.
#[derive(Debug)]
pub struct AcknowledgeMatchDisagreementTool {
    port: Arc<dyn MatchReviewPort>,
}

/// `link_match_disagreement`: link one disagreement to an issue reference.
#[derive(Debug)]
pub struct LinkMatchDisagreementTool {
    port: Arc<dyn MatchReviewPort>,
}

impl SubmitMatchReviewTool {
    /// A handler over the review port.
    pub fn new(port: Arc<dyn MatchReviewPort>) -> Self {
        Self { port }
    }
}

impl GetMatchReviewTool {
    /// A handler over the review port.
    pub fn new(port: Arc<dyn MatchReviewPort>) -> Self {
        Self { port }
    }
}

impl ListMatchReviewsTool {
    /// A handler over the review port.
    pub fn new(port: Arc<dyn MatchReviewPort>) -> Self {
        Self { port }
    }
}

impl ListMatchDisagreementsTool {
    /// A handler over the review port.
    pub fn new(port: Arc<dyn MatchReviewPort>) -> Self {
        Self { port }
    }
}

impl AcknowledgeMatchDisagreementTool {
    /// A handler over the review port.
    pub fn new(port: Arc<dyn MatchReviewPort>) -> Self {
        Self { port }
    }
}

impl LinkMatchDisagreementTool {
    /// A handler over the review port.
    pub fn new(port: Arc<dyn MatchReviewPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for SubmitMatchReviewTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::submit_match_review(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.submit_match_review(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for GetMatchReviewTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::get_match_review(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.get_match_review(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for ListMatchReviewsTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::list_match_reviews(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.list_match_reviews(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for ListMatchDisagreementsTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::list_match_disagreements(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.list_match_disagreements(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for AcknowledgeMatchDisagreementTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::acknowledge_match_disagreement(arguments) {
            Ok(request) => ToolResult::from_outcome(
                self.port.acknowledge_match_disagreement(&request).await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for LinkMatchDisagreementTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::link_match_disagreement(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.link_match_disagreement(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the review tools whose port is supplied. An absent port leaves all six unregistered, as
/// for a server without a review service.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn MatchReviewPort>>,
) -> Result<(), RegistryError> {
    let Some(port) = port else {
        return Ok(());
    };
    registry.register(
        SUBMIT_MATCH_REVIEW,
        SubmitMatchReviewTool::new(Arc::clone(&port)),
    )?;
    registry.register(GET_MATCH_REVIEW, GetMatchReviewTool::new(Arc::clone(&port)))?;
    registry.register(
        LIST_MATCH_REVIEWS,
        ListMatchReviewsTool::new(Arc::clone(&port)),
    )?;
    registry.register(
        LIST_MATCH_DISAGREEMENTS,
        ListMatchDisagreementsTool::new(Arc::clone(&port)),
    )?;
    registry.register(
        ACKNOWLEDGE_MATCH_DISAGREEMENT,
        AcknowledgeMatchDisagreementTool::new(Arc::clone(&port)),
    )?;
    registry.register(
        LINK_MATCH_DISAGREEMENT,
        LinkMatchDisagreementTool::new(port),
    )
}
