//! The bundled catalog must equal the recorded `tools/list` golden, tool by tool and in order, and a
//! server lists only the tools whose handlers are registered, in the catalog order.

mod common;

use std::error::Error;
use std::sync::Arc;

use common::test_tools::{Script, ScriptedTool};
use jobniac_mcp::JobniacMcpServer;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::ToolRegistry;
use serde_json::Value;

#[test]
fn bundled_catalog_equals_the_golden_tool_list() -> Result<(), Box<dyn Error>> {
    let catalog = Catalog::load()?;
    let actual: Vec<Value> = catalog
        .tools()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    let expected = common::golden(include_str!("golden/tools-list-full.json"))?;
    assert_eq!(Value::Array(actual), expected);
    Ok(())
}

#[tokio::test]
async fn listing_shows_only_registered_tools_in_catalog_order() -> Result<(), Box<dyn Error>> {
    let script = Arc::new(Script::default());
    let mut registry = ToolRegistry::new(Catalog::load()?);
    // Registered in reverse order: the listing must still follow the catalog.
    registry.register(
        "search_jobs",
        ScriptedTool::new("search_jobs", Arc::clone(&script)),
    )?;
    registry.register(
        "list_profiles",
        ScriptedTool::new("list_profiles", Arc::clone(&script)),
    )?;
    assert!(registry.is_registered("search_jobs"));
    assert!(!registry.is_registered("get_profile"));

    let client = common::connect_server(JobniacMcpServer::with_registry(registry)).await?;
    let tools = client.list_all_tools().await?;
    let names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["list_profiles".to_owned(), "search_jobs".to_owned()]
    );
    Ok(())
}

#[test]
fn registry_refuses_unknown_and_duplicate_names() -> Result<(), Box<dyn Error>> {
    let script = Arc::new(Script::default());
    let mut registry = ToolRegistry::new(Catalog::load()?);
    assert!(
        registry
            .register(
                "not_in_catalog",
                ScriptedTool::new("get_profile", Arc::clone(&script))
            )
            .is_err()
    );
    registry.register(
        "get_profile",
        ScriptedTool::new("get_profile", Arc::clone(&script)),
    )?;
    assert!(
        registry
            .register(
                "get_profile",
                ScriptedTool::new("get_profile", Arc::clone(&script))
            )
            .is_err()
    );
    Ok(())
}
