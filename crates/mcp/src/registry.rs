//! The typed tool registry: handlers by catalog name, listed in catalog order.
//!
//! A tool module implements [`ToolHandler`] and registers it under the tool's catalog name. The
//! registry refuses a name that the catalog does not define and a second handler for one name, so
//! a typo or a duplicate fails at composition time. [`ToolRegistry::listed_tools`] lists only the
//! tools that have a handler, and [`ToolRegistry::call`] answers `None` for every other name, which
//! the server reports as `Unknown tool`, as the SDK does for an unregistered tool.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::Tool;
use serde_json::{Map, Value};

use crate::args::{ArgErrors, IssueKind};
use crate::catalog::Catalog;
use crate::result::ToolResult;

/// The arguments of one tool call: the JSON object the client sent (an absent object is empty).
pub type Arguments = Map<String, Value>;

/// A tool implementation. Argument decoding happens inside, with the helpers of
/// [`crate::args`]; a rejected argument set is returned as [`ToolResult::InvalidArguments`].
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Run the tool with the wire arguments of one call.
    async fn call(&self, arguments: &Arguments) -> ToolResult;
}

/// A registration that the catalog or the registry does not allow.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The name is not defined in the catalog.
    #[error("tool {0} is not defined in the catalog")]
    NotInCatalog(String),
    /// A handler is already registered under the name.
    #[error("tool {0} is already registered")]
    Duplicate(String),
}

/// The tool handlers of one server, over the catalog they are listed from.
pub struct ToolRegistry {
    catalog: Catalog,
    handlers: BTreeMap<String, Arc<dyn ToolHandler>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("catalog", &self.catalog)
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    /// An empty registry over `catalog`.
    pub const fn new(catalog: Catalog) -> Self {
        Self {
            catalog,
            handlers: BTreeMap::new(),
        }
    }

    /// The catalog this registry lists from.
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Register `handler` for the catalog tool `name`.
    pub fn register<H>(&mut self, name: &str, handler: H) -> Result<(), RegistryError>
    where
        H: ToolHandler + 'static,
    {
        if self.catalog.tool(name).is_none() {
            return Err(RegistryError::NotInCatalog(name.to_owned()));
        }
        if self.handlers.contains_key(name) {
            return Err(RegistryError::Duplicate(name.to_owned()));
        }
        self.handlers.insert(name.to_owned(), Arc::new(handler));
        Ok(())
    }

    /// Whether a handler is registered under `name`.
    pub fn is_registered(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// The definitions of the registered tools, in catalog order.
    pub fn listed_tools(&self) -> Vec<Tool> {
        self.catalog
            .tools()
            .iter()
            .filter(|tool| self.handlers.contains_key(tool.name.as_ref()))
            .cloned()
            .collect()
    }

    /// Run the tool `name`, or answer `None` when no handler is registered under that name.
    pub async fn call(&self, name: &str, arguments: &Arguments) -> Option<ToolResult> {
        let handler = self.handlers.get(name)?.clone();
        Some(handler.call(arguments).await)
    }

    /// Answer a call of the registered tool `name` with the argument rejection `kind`, without
    /// running its handler (input that could not be admitted at stdin, see
    /// [`crate::stdin_admission`]). `None` when no handler is registered under that name.
    pub fn reject_input(&self, name: &str, kind: IssueKind) -> Option<ToolResult> {
        self.handlers
            .contains_key(name)
            .then(|| ToolResult::from(ArgErrors::input_rejection(name, kind)))
    }
}
