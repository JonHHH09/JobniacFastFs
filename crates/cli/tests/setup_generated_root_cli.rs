//! Black-box tests of `setup-generated-root` that stop before any database or filesystem work: the
//! preconditions, their fixed texts and the exit status. Every process starts with a cleared
//! environment. The synthetic database settings point at a loopback address that is never
//! contacted, because each refusal below comes before the schema check.

use std::error::Error;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn Error>>;

const PREFIX: &str = "Jobniac setup-generated-root: ";
#[cfg(windows)]
const ROOT: &str = "C:\\synthetic\\generated-root-marker";
#[cfg(not(windows))]
const ROOT: &str = "/synthetic/generated-root-marker";
const DATABASE: &[(&str, &str)] = &[
    ("JOBNIAC_DB_HOST", "127.0.0.1"),
    ("JOBNIAC_DB_PORT", "1"),
    ("JOBNIAC_DB_NAME", "jobniac_synthetic"),
    ("JOBNIAC_DB_USER", "synthetic_user"),
    ("JOBNIAC_DB_PASSWORD", "synthetic_password"),
];

/// A command for the built binary with an empty environment.
fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command.env_clear();
    command
}

fn setup(variables: &[(&str, &str)]) -> std::io::Result<Output> {
    let mut command = cli();
    command.arg("setup-generated-root");
    for (key, value) in variables {
        command.env(key, value);
    }
    command.output()
}

fn with_database(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    DATABASE
        .iter()
        .copied()
        .chain(extra.iter().copied())
        .collect()
}

fn assert_refusal(output: &Output, text: &str) -> TestResult {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "a refusal prints no status");
    assert_eq!(
        String::from_utf8(output.stderr.clone())?,
        format!("{PREFIX}{text}\n")
    );
    Ok(())
}

#[test]
fn a_missing_database_is_refused_first() -> TestResult {
    let output = setup(&[("JOBNIAC_GENERATED_ROOT", ROOT)])?;
    assert_refusal(
        &output,
        "database is not configured; set the JOBNIAC_DB_* variables.",
    )
}

#[test]
fn a_missing_root_is_refused_before_the_identity_is_read() -> TestResult {
    let output = setup(&with_database(&[(
        "JOBNIAC_GENERATED_ROOT_ID",
        "not-a-uuid",
    )]))?;
    assert_refusal(
        &output,
        "the generated root path is not configured or invalid.",
    )
}

#[test]
fn a_relative_root_is_refused_as_not_configured() -> TestResult {
    let output = setup(&with_database(&[(
        "JOBNIAC_GENERATED_ROOT",
        "relative/root",
    )]))?;
    assert_refusal(
        &output,
        "the generated root path is not configured or invalid.",
    )
}

#[test]
fn a_malformed_identity_is_refused_before_the_schema_is_inspected() -> TestResult {
    for identity in [
        "6F1C2D3E-4A5B-4C6D-8E7F-900112233445",
        "{6f1c2d3e-4a5b-4c6d-8e7f-900112233445}",
        "6f1c2d3e4a5b4c6d8e7f900112233445",
    ] {
        let output = setup(&with_database(&[
            ("JOBNIAC_GENERATED_ROOT", ROOT),
            ("JOBNIAC_GENERATED_ROOT_ID", identity),
        ]))?;
        assert_refusal(&output, "JOBNIAC_GENERATED_ROOT_ID is invalid.")?;
    }
    Ok(())
}

#[test]
fn the_refusals_never_print_the_root_path() -> TestResult {
    let output = setup(&with_database(&[
        ("JOBNIAC_GENERATED_ROOT", ROOT),
        ("JOBNIAC_GENERATED_ROOT_ID", "not-a-uuid"),
    ]))?;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(!stderr.contains(ROOT));
    Ok(())
}
