//! Wire definitions of the MCP tools and the server identity, stored as JSON data.
//!
//! `tools.json` holds all 40 tool definitions in their registration order
//! (`tools/list` order). It is the exact wire form of each tool's `name`, `title`, `description`,
//! `inputSchema`, `outputSchema` and `annotations`, copied from the golden
//! `tests/golden/tools-list-full.json`; the catalog test checks that it still equals that golden.
//! A tool is listed only when a handler is registered for it, so the catalog itself never decides
//! what a server exposes.

use std::collections::BTreeMap;

use rmcp::model::{InitializeResult, Tool};

/// All tool definitions in registration order.
const TOOLS_JSON: &str = include_str!("tools.json");

/// The `initialize` result: protocol version, capabilities and `serverInfo`.
const INITIALIZE_JSON: &str = include_str!("initialize.json");

/// The parsed catalog: tool definitions (in order, addressable by name) and the server identity.
#[derive(Debug, Clone)]
pub struct Catalog {
    tools: Vec<Tool>,
    positions: BTreeMap<String, usize>,
    initialize: InitializeResult,
}

impl Catalog {
    /// Parse the bundled catalog.
    pub fn load() -> Result<Self, serde_json::Error> {
        let tools: Vec<Tool> = serde_json::from_str(TOOLS_JSON)?;
        let initialize = serde_json::from_str(INITIALIZE_JSON)?;
        Ok(Self::from_parts(tools, initialize))
    }

    fn from_parts(tools: Vec<Tool>, initialize: InitializeResult) -> Self {
        let positions = tools
            .iter()
            .enumerate()
            .map(|(position, tool)| (tool.name.to_string(), position))
            .collect();
        Self {
            tools,
            positions,
            initialize,
        }
    }

    /// Every tool definition, in registration order.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The definition of the tool with this name, if the catalog has one.
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.positions
            .get(name)
            .and_then(|position| self.tools.get(*position))
    }

    /// The `initialize` result of the server.
    pub fn initialize(&self) -> &InitializeResult {
        &self.initialize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_all_forty_tools_once_in_order() -> Result<(), serde_json::Error> {
        let catalog = Catalog::load()?;
        assert_eq!(catalog.tools().len(), 40);
        assert_eq!(catalog.positions.len(), 40);
        assert_eq!(catalog.tools()[0].name, "health");
        assert_eq!(catalog.tools()[39].name, "add_job_from_link");
        Ok(())
    }

    #[test]
    fn tools_are_addressable_by_name() -> Result<(), serde_json::Error> {
        let catalog = Catalog::load()?;
        let update = catalog
            .tool("update_profile")
            .map(|tool| tool.name.to_string());
        assert_eq!(update.as_deref(), Some("update_profile"));
        assert!(catalog.tool("does_not_exist").is_none());
        Ok(())
    }

    #[test]
    fn initialize_result_names_the_server() -> Result<(), serde_json::Error> {
        let catalog = Catalog::load()?;
        assert_eq!(catalog.initialize().server_info.name, "Jobniac");
        Ok(())
    }
}
