//! The deletion tools are composed only by the root composition (`JOBNIAC_DELETION_ENABLED` exactly
//! `1` and a database, both from the one environment snapshot). The composition is the real
//! `jobniac_runtime::services::compose`; no database connection is made.

#[path = "../src/deletion_ports.rs"]
#[allow(dead_code)]
mod deletion_ports;

use std::error::Error;

use jobniac_core::env::Environment;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::ToolRegistry;
use jobniac_mcp::tools::deletion;
use jobniac_runtime::services::compose;

type TestResult = Result<(), Box<dyn Error>>;

const DATABASE: [(&str, &str); 5] = [
    ("JOBNIAC_DB_HOST", "127.0.0.1"),
    ("JOBNIAC_DB_PORT", "5432"),
    ("JOBNIAC_DB_NAME", "jobniac_test"),
    ("JOBNIAC_DB_USER", "app_user"),
    ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
];

/// The names of the deletion tools that the composed environment registers.
fn deletion_tools(pairs: &[(&str, &str)]) -> Result<Vec<String>, Box<dyn Error>> {
    let env = Environment::from_pairs(pairs.to_vec());
    let services = compose(&env).map_err(|_| "startup was refused")?;
    let mut registry = ToolRegistry::new(Catalog::load()?);
    deletion::register(
        &mut registry,
        deletion_ports::from_services(&services.deletion),
    )?;
    Ok(registry
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect())
}

fn with_database(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    DATABASE
        .iter()
        .copied()
        .chain(extra.iter().copied())
        .collect()
}

#[test]
fn the_exact_flag_with_a_database_registers_both_tools() -> TestResult {
    let tools = deletion_tools(&with_database(&[("JOBNIAC_DELETION_ENABLED", "1")]))?;
    assert_eq!(tools, vec!["delete_profile", "delete_job"]);
    Ok(())
}

#[test]
fn any_other_flag_value_registers_nothing() -> TestResult {
    for value in ["true", "yes", "0", "01", " 1", "1 ", ""] {
        let tools = deletion_tools(&with_database(&[("JOBNIAC_DELETION_ENABLED", value)]))?;
        assert!(
            tools.is_empty(),
            "flag {value:?} must not compose the tools"
        );
    }
    Ok(())
}

#[test]
fn the_flag_without_a_database_registers_nothing() -> TestResult {
    let tools = deletion_tools(&[("JOBNIAC_DELETION_ENABLED", "1")])?;
    assert!(tools.is_empty());
    Ok(())
}

#[test]
fn a_database_without_the_flag_registers_nothing() -> TestResult {
    let tools = deletion_tools(&DATABASE)?;
    assert!(tools.is_empty());
    Ok(())
}
