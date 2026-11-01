//! The desktop defaults compose the Arbeitnow services: a first run with no variable set generates
//! the keys file, and composition over the effective environment finds the flag, the keys and the
//! database. This is the path the desktop shell takes at startup (`prepare_environment`, then
//! `compose_arbeitnow`), without the window.

use std::ffi::OsString;

use jobniac_runtime::app_settings::FeatureSettings;
use jobniac_runtime::arbeitnow::{FLAG_KEY, compose_arbeitnow};
use jobniac_runtime::database::prepare;
use jobniac_runtime::desktop_features::{KEYS_FILE, prepare_environment};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn pairs(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    entries
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

#[tokio::test]
async fn a_first_run_without_variables_composes_arbeitnow() -> TestResult {
    let dir = tempfile::tempdir()?;
    let database = dir.path().join("jobniac.sqlite3");
    let keys = dir.path().join("data");
    let database_text = database.to_str().ok_or("temporary path is not Unicode")?;
    let home = dir.path().to_str().ok_or("temporary path is not Unicode")?;
    let prepared = prepare_environment(
        pairs(&[
            ("JOBNIAC_DB_PATH", database_text),
            ("HOME", home),
            ("LOCALAPPDATA", home),
            ("XDG_DATA_HOME", home),
        ]),
        &FeatureSettings::default(),
        Some(&keys),
    )
    .await;
    assert!(prepared.warnings.is_empty(), "{:?}", prepared.warnings);
    assert!(keys.join(KEYS_FILE).is_file());
    let env = prepared.environment();
    assert!(env.flag_enabled(FLAG_KEY));
    let pool = prepare(&database).ok_or("the database must open")?;
    let services = compose_arbeitnow(&env, Some(&pool))?;
    assert!(services.is_some(), "Arbeitnow must be composed by default");
    Ok(())
}
