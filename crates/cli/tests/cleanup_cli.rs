//! Black-box tests of the `cleanup` command without a database: the refusal text and exit status
//! when the generated lifecycle cannot run. Every process starts with a cleared environment, and
//! the synthetic root is never created. No test connects to a database.

use std::error::Error;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn Error>>;

const REFUSAL: &str = "Jobniac cleanup: configured generation lifecycle is unavailable.\n";

/// A command for the built binary with an empty environment.
fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command.env_clear();
    command
}

fn cleanup(variables: &[(&str, &str)]) -> std::io::Result<Output> {
    let mut command = cli();
    command.arg("cleanup");
    for (key, value) in variables {
        command.env(key, value);
    }
    command.output()
}

#[test]
fn an_empty_environment_refuses_with_the_fixed_text() -> TestResult {
    let output = cleanup(&[])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no metadata without a run");
    assert_eq!(String::from_utf8(output.stderr)?, REFUSAL);
    Ok(())
}

#[test]
fn a_flag_other_than_exactly_one_never_admits_the_lifecycle() -> TestResult {
    for flag in ["true", "0", " 1", "01"] {
        let output = cleanup(&[
            ("JOBNIAC_GENERATED_ENABLED", flag),
            ("JOBNIAC_GENERATED_ROOT", "/synthetic/generated"),
            (
                "JOBNIAC_GENERATED_ROOT_ID",
                "6f1c2d3e-4a5b-4c6d-8e7f-900112233445",
            ),
        ])?;
        assert_eq!(output.status.code(), Some(1), "flag {flag:?}");
        assert!(output.stdout.is_empty(), "flag {flag:?}");
        assert_eq!(String::from_utf8(output.stderr)?, REFUSAL, "flag {flag:?}");
    }
    Ok(())
}

#[test]
fn an_admitted_root_without_a_database_still_refuses_without_a_run() -> TestResult {
    let output = cleanup(&[
        ("JOBNIAC_GENERATED_ENABLED", "1"),
        ("JOBNIAC_GENERATED_ROOT", "/synthetic/generated"),
        (
            "JOBNIAC_GENERATED_ROOT_ID",
            "6f1c2d3e-4a5b-4c6d-8e7f-900112233445",
        ),
    ])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr)?, REFUSAL);
    Ok(())
}

#[test]
fn the_refusal_never_echoes_the_root_path() -> TestResult {
    let root = "/synthetic/private-root-marker";
    let output = cleanup(&[
        ("JOBNIAC_GENERATED_ENABLED", "1"),
        ("JOBNIAC_GENERATED_ROOT", root),
        ("JOBNIAC_GENERATED_ROOT_ID", "not-a-uuid"),
    ])?;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    let stdout = String::from_utf8(output.stdout)?;
    assert!(!stderr.contains(root) && !stdout.contains(root));
    Ok(())
}
