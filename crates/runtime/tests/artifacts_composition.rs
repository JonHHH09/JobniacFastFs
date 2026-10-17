//! Composition of the opt-in document artifacts and its flag rules. Composition performs no
//! database access. All values are synthetic.

use std::error::Error;

use jobniac_core::env::Environment;
use jobniac_runtime::services::compose;

type TestResult = Result<(), Box<dyn Error>>;

const DATABASE_PAIRS: [(&str, &str); 5] = [
    ("JOBNIAC_DB_HOST", "127.0.0.1"),
    ("JOBNIAC_DB_PORT", "5432"),
    ("JOBNIAC_DB_NAME", "jobniac_test"),
    ("JOBNIAC_DB_USER", "app_user"),
    ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
];

fn with_flag(flag: Option<&'static str>, database: bool) -> Environment {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    if database {
        pairs.extend(DATABASE_PAIRS);
    }
    if let Some(value) = flag {
        pairs.push(("JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED", value));
    }
    Environment::from_pairs(pairs)
}

#[test]
fn the_default_composition_has_no_artifact_services() -> TestResult {
    let services = compose(&with_flag(None, true))?;
    assert!(services.artifacts.is_none());
    Ok(())
}

#[test]
fn only_the_exact_value_one_enables_the_services() -> TestResult {
    for value in ["true", "yes", "0", "01", "", " 1"] {
        let services = compose(&with_flag(Some(value), true))?;
        assert!(
            services.artifacts.is_none(),
            "flag value {value:?} enabled the services"
        );
    }
    Ok(())
}

#[test]
fn the_flag_without_a_database_has_no_effect() -> TestResult {
    let services = compose(&with_flag(Some("1"), false))?;
    assert!(services.artifacts.is_none());
    Ok(())
}

#[test]
fn the_exact_flag_with_a_database_composes_the_read_and_the_exporter() -> TestResult {
    let services = compose(&with_flag(Some("1"), true))?;
    assert!(services.artifacts.is_some());
    Ok(())
}
