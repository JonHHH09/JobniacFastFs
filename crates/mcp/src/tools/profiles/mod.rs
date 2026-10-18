//! The profile tools: `list_profiles`, `get_profile`, `create_profile`, `update_profile`,
//! `update_profile_project` and `search_profiles`.
//!
//! Each tool decodes its arguments in the shape of its signature (see [`decode`]) and then
//! calls one port. A port answers with the service result, as a JSON
//! value, or with an [`AppError`]. The ports are supplied by the composition root (the CLI), so
//! this crate does not depend on the SQLite adapter; an absent port means an absent tool.
//! Every profile tool reports its errors with the compact envelope.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::args::request_argument;
use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `list_profiles`.
pub const LIST_PROFILES: &str = "list_profiles";
/// The catalog name of `get_profile`.
pub const GET_PROFILE: &str = "get_profile";
/// The catalog name of `create_profile`.
pub const CREATE_PROFILE: &str = "create_profile";
/// The catalog name of `update_profile`.
pub const UPDATE_PROFILE: &str = "update_profile";
/// The catalog name of `update_profile_project`.
pub const UPDATE_PROFILE_PROJECT: &str = "update_profile_project";
/// The catalog name of `search_profiles`.
pub const SEARCH_PROFILES: &str = "search_profiles";

/// The envelope style of every profile tool (`profile_mcp._call` and `list_profiles`).
const STYLE: EnvelopeStyle = EnvelopeStyle::Compact;

/// The keyset listing of profile identities (`ProfileListService`).
#[async_trait]
pub trait ProfileListPort: std::fmt::Debug + Send + Sync {
    /// One page of identities: the service dictionary with `profiles` and `nextCursor`.
    async fn list_profiles(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<Value, AppError>;
}

/// The profile aggregate use cases (`ProfileDetailsService`).
#[async_trait]
pub trait ProfileDetailsPort: std::fmt::Debug + Send + Sync {
    /// The complete graph of one profile. `profile_id` is the normalized text of the argument.
    async fn get_profile(&self, profile_id: &str) -> Result<Value, AppError>;
    /// Create a profile from the `ProfileWriteRequest` object.
    async fn create_profile(&self, request: &Value) -> Result<Value, AppError>;
    /// Replace a profile's graph when `expected_revision` matches the stored revision.
    async fn update_profile(
        &self,
        profile_id: &str,
        expected_revision: i64,
        request: &Value,
    ) -> Result<Value, AppError>;
    /// Patch one project from the `ProjectUpdateRequest` object.
    async fn update_profile_project(&self, request: &Value) -> Result<Value, AppError>;
}

/// The bounded profile search (`ProfileSearchService`).
#[async_trait]
pub trait ProfileSearchPort: std::fmt::Debug + Send + Sync {
    /// Search from the `ProfileSearchRequest` object and return the search envelope.
    async fn search_profiles(&self, request: &Value) -> Result<Value, AppError>;
}

/// `list_profiles`: one page of profile identities.
#[derive(Debug)]
pub struct ListProfilesTool {
    port: Arc<dyn ProfileListPort>,
}

/// `get_profile`: one profile graph by its identifier.
#[derive(Debug)]
pub struct GetProfileTool {
    port: Arc<dyn ProfileDetailsPort>,
}

/// `create_profile`: create a profile graph.
#[derive(Debug)]
pub struct CreateProfileTool {
    port: Arc<dyn ProfileDetailsPort>,
}

/// `update_profile`: replace a profile graph under the revision check.
#[derive(Debug)]
pub struct UpdateProfileTool {
    port: Arc<dyn ProfileDetailsPort>,
}

/// `update_profile_project`: patch one project under the revision check.
#[derive(Debug)]
pub struct UpdateProfileProjectTool {
    port: Arc<dyn ProfileDetailsPort>,
}

/// `search_profiles`: bounded search over the profile posting index.
#[derive(Debug)]
pub struct SearchProfilesTool {
    port: Arc<dyn ProfileSearchPort>,
}

impl ListProfilesTool {
    /// A handler over the listing port.
    pub fn new(port: Arc<dyn ProfileListPort>) -> Self {
        Self { port }
    }
}

impl GetProfileTool {
    /// A handler over the details port.
    pub fn new(port: Arc<dyn ProfileDetailsPort>) -> Self {
        Self { port }
    }
}

impl CreateProfileTool {
    /// A handler over the details port.
    pub fn new(port: Arc<dyn ProfileDetailsPort>) -> Self {
        Self { port }
    }
}

impl UpdateProfileTool {
    /// A handler over the details port.
    pub fn new(port: Arc<dyn ProfileDetailsPort>) -> Self {
        Self { port }
    }
}

impl UpdateProfileProjectTool {
    /// A handler over the details port.
    pub fn new(port: Arc<dyn ProfileDetailsPort>) -> Self {
        Self { port }
    }
}

impl SearchProfilesTool {
    /// A handler over the search port.
    pub fn new(port: Arc<dyn ProfileSearchPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for ListProfilesTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::list_profiles(arguments) {
            Ok(list) => ToolResult::from_outcome(
                self.port
                    .list_profiles(list.limit, list.cursor.as_deref())
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for GetProfileTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::get_profile(arguments) {
            Ok(profile_id) => {
                ToolResult::from_outcome(self.port.get_profile(&profile_id).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for CreateProfileTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::create_profile(arguments) {
            Ok(()) => ToolResult::from_outcome(
                self.port.create_profile(&request_argument(arguments)).await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for UpdateProfileTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::update_profile(arguments) {
            Ok((profile_id, expected_revision)) => ToolResult::from_outcome(
                self.port
                    .update_profile(&profile_id, expected_revision, &request_argument(arguments))
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for UpdateProfileProjectTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::update_profile_project(arguments) {
            Ok(()) => ToolResult::from_outcome(
                self.port
                    .update_profile_project(&request_argument(arguments))
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for SearchProfilesTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::search_profiles(arguments) {
            Ok(()) => ToolResult::from_outcome(
                self.port
                    .search_profiles(&request_argument(arguments))
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the profile tools whose ports are supplied. Each group is optional: the
/// listing, the details (four tools) and the search register separately.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    list: Option<Arc<dyn ProfileListPort>>,
    details: Option<Arc<dyn ProfileDetailsPort>>,
    search: Option<Arc<dyn ProfileSearchPort>>,
) -> Result<(), RegistryError> {
    if let Some(port) = list {
        registry.register(LIST_PROFILES, ListProfilesTool::new(port))?;
    }
    if let Some(port) = details {
        registry.register(GET_PROFILE, GetProfileTool::new(Arc::clone(&port)))?;
        registry.register(CREATE_PROFILE, CreateProfileTool::new(Arc::clone(&port)))?;
        registry.register(UPDATE_PROFILE, UpdateProfileTool::new(Arc::clone(&port)))?;
        registry.register(UPDATE_PROFILE_PROJECT, UpdateProfileProjectTool::new(port))?;
    }
    if let Some(port) = search {
        registry.register(SEARCH_PROFILES, SearchProfilesTool::new(port))?;
    }
    Ok(())
}
