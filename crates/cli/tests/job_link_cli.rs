//! The link tool is composed only when `JOBNIAC_LINK_FETCH_ENABLED` is exactly `1`, a database is
//! configured and the platform is supported. Composition spawns nothing and fetches nothing. The
//! database is a temporary file. All data is synthetic.

use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::env::Environment;
use jobniac_core::errors::AppError;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::ToolRegistry;
use jobniac_mcp::tools::job_links::{self, JobLinkPort};
use jobniac_runtime::job_links::compose_job_links;
use jobniac_testkit::sqlite::TempDatabase;
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

const FLAG: &str = "JOBNIAC_LINK_FETCH_ENABLED";

/// A port that is only needed to register the tool; it is never called here.
#[derive(Debug)]
struct Unused;

#[async_trait]
impl JobLinkPort for Unused {
    async fn add_job_from_link(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

/// The composed use case for the given settings, with or without the database.
fn composed(extra: &[(&str, &str)], with_database: bool) -> Result<bool, Box<dyn Error>> {
    let temporary = TempDatabase::new()?;
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    if with_database {
        pairs.push(temporary.environment_pair());
    }
    pairs.extend_from_slice(extra);
    let env = Environment::from_pairs(pairs);
    let database = jobniac_runtime::database::open_database(&env);
    let service = compose_job_links(&env, database.as_ref()).map_err(|_| "startup was refused")?;
    Ok(service.is_some())
}

/// The names of the tools registered for an optional port.
fn listed(port: Option<Arc<dyn JobLinkPort>>) -> Result<Vec<String>, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    job_links::register(&mut registry, port)?;
    Ok(registry
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect())
}

#[test]
fn only_the_exact_flag_with_a_database_composes_the_use_case() -> TestResult {
    assert!(composed(&[(FLAG, "1")], true)?);
    for value in ["0", "", "true", "01", "yes", " 1", "1 "] {
        assert!(!composed(&[(FLAG, value)], true)?, "flag {value:?}");
    }
    assert!(!composed(&[], true)?, "the default is off");
    Ok(())
}

#[test]
fn the_flag_without_a_database_composes_nothing() -> TestResult {
    assert!(!composed(&[(FLAG, "1")], false)?);
    Ok(())
}

#[test]
fn the_tool_is_listed_only_when_its_port_is_present() -> TestResult {
    assert!(listed(None)?.is_empty());
    let port: Arc<dyn JobLinkPort> = Arc::new(Unused);
    assert_eq!(listed(Some(port))?, vec!["add_job_from_link"]);
    Ok(())
}
