//! Composition of the opt-in document artifacts and its flag rules. The database is a temporary
//! file. All values are synthetic.

use std::error::Error;

use jobniac_core::env::Environment;
use jobniac_runtime::services::compose;

type TestResult = Result<(), Box<dyn Error>>;

/// A temporary database location: the directory guard and the file path as text.
fn database_file() -> Result<(tempfile::TempDir, String), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("jobniac.sqlite3");
    let text = file
        .to_str()
        .ok_or("temporary path is not Unicode")?
        .to_owned();
    Ok((root, text))
}

fn with_flag(flag: Option<&'static str>, database: Option<&str>) -> Environment {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    if let Some(path) = database {
        pairs.push(("JOBNIAC_DB_PATH", path));
    }
    if let Some(value) = flag {
        pairs.push(("JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED", value));
    }
    Environment::from_pairs(pairs)
}

#[test]
fn the_default_composition_has_no_artifact_services() -> TestResult {
    let (_root, path) = database_file()?;
    let services = compose(&with_flag(None, Some(&path)))?;
    assert!(services.artifacts.is_none());
    Ok(())
}

#[test]
fn only_the_exact_value_one_enables_the_services() -> TestResult {
    let (_root, path) = database_file()?;
    for value in ["true", "yes", "0", "01", "", " 1"] {
        let services = compose(&with_flag(Some(value), Some(&path)))?;
        assert!(
            services.artifacts.is_none(),
            "flag value {value:?} enabled the services"
        );
    }
    Ok(())
}

#[test]
fn the_flag_without_a_database_has_no_effect() -> TestResult {
    let services = compose(&with_flag(Some("1"), None))?;
    assert!(services.artifacts.is_none());
    Ok(())
}

#[test]
fn the_exact_flag_with_a_database_composes_the_read_and_the_exporter() -> TestResult {
    let (_root, path) = database_file()?;
    let services = compose(&with_flag(Some("1"), Some(&path)))?;
    assert!(services.artifacts.is_some());
    Ok(())
}
