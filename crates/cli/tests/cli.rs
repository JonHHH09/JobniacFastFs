//! Black-box tests of the `jobniac-cli` binary.
//!
//! Every process starts with a cleared environment, so no database setting or feature flag can
//! leak in from the machine running the tests. No test connects to a database.

use std::error::Error;
use std::io::Write;
use std::process::{Command, Output, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

/// A command for the built binary with an empty environment.
fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command.env_clear();
    command
}

fn run(args: &[&str]) -> std::io::Result<Output> {
    cli().args(args).output()
}

#[test]
fn version_prints_the_crate_version_to_stdout() -> TestResult {
    let output = run(&["--version"])?;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!("Jobniac {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn help_describes_every_mode_as_available() -> TestResult {
    let output = run(&["--help"])?;
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8(output.stdout)?;
    assert!(
        !help.to_lowercase().contains("not available"),
        "every mode is implemented: {help}"
    );
    assert!(help.contains("cleanup pass"), "{help}");
    assert!(help.contains("idempotent"), "{help}");
    Ok(())
}

#[test]
fn unknown_mode_is_a_usage_error() -> TestResult {
    let output = run(&["unknown-mode"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn missing_mode_is_a_usage_error() -> TestResult {
    let output = run(&[])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn desktop_is_not_a_mode_of_the_console_binary() -> TestResult {
    let output = run(&["desktop"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn migrate_without_database_configuration_reports_the_setting_names() -> TestResult {
    let output = run(&["migrate"])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim_end(),
        "Jobniac migrate: database is not configured; set the JOBNIAC_DB_* variables."
    );
    Ok(())
}

#[test]
fn http_without_loopback_settings_reports_the_sanitized_failure() -> TestResult {
    // The listener needs JOBNIAC_HTTP_PORT and JOBNIAC_HTTP_BEARER_TOKEN; without them it refuses
    // before composing anything. The full http mode is covered by tests/http_cli.rs.
    let output = run(&["http"])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr)?.trim_end(),
        "Jobniac could not start. Check local configuration."
    );
    Ok(())
}

#[test]
fn mcp_answers_initialize_and_tools_list_on_stdout_only() -> TestResult {
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic-client","version":"0.0.0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ];
    let mut child = cli()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or("stdin was not piped")?;
        for request in requests {
            writeln!(stdin, "{request}")?;
        }
        // Dropping stdin closes it, which ends the session.
    }
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0));

    let messages: Vec<serde_json::Value> = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(
        messages.len(),
        2,
        "stdout must carry only the two responses"
    );

    let initialize = &messages[0];
    assert_eq!(initialize["id"], 1);
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(initialize["result"]["serverInfo"]["name"], "Jobniac");

    let listing = &messages[1];
    assert_eq!(listing["id"], 2);
    let names: Vec<&str> = listing["result"]["tools"]
        .as_array()
        .ok_or("tools is not an array")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    // The default composition registers `health`, the profile, job, matching, review and document
    // tools; the CLI test runs with no database, so those tools are listed
    // but every call reports the unconfigured database.
    assert_eq!(
        names,
        [
            "health",
            "list_profiles",
            "get_profile",
            "create_profile",
            "update_profile",
            "update_profile_project",
            "search_profiles",
            "search_jobs",
            "update_job",
            "list_jobs",
            "get_job",
            "add_job_from_text",
            "analyze_job_match",
            "analyze_all_job_matches",
            "get_match_report",
            "list_match_reports",
            "submit_match_review",
            "get_match_review",
            "list_match_reviews",
            "list_match_disagreements",
            "acknowledge_match_disagreement",
            "link_match_disagreement",
            "store_document_file",
            "get_document_metadata",
            "extract_pdf_text",
            "extract_stored_pdf_text",
            "ingest_profile_from_stored_pdf",
            "get_profile_pdf_source",
            "generate_pdf_file",
            "generate_pdf_resume",
            "generate_canadian_pdf_resume",
            "generate_canadian_french_pdf_resume",
            "generate_german_tailored_resume",
            "generate_german_cover_letter",
        ]
    );
    Ok(())
}

#[test]
fn mcp_with_stdin_closed_before_initialize_exits_zero_silently() -> TestResult {
    let mut child = cli()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stdin.take());
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    Ok(())
}

// The catalog per configuration, through the real binary: `tools/list` of `jobniac-cli mcp` must
// list the tool names of each opt-in configuration, in their catalog order, and a flag value
// other than the exact `1` must compose nothing. The composition needs the database settings and
// the Arbeitnow keys (synthetic values below); no test connects to a database.

const DATABASE_PAIRS: [(&str, &str); 5] = [
    ("JOBNIAC_DB_HOST", "127.0.0.1"),
    ("JOBNIAC_DB_PORT", "5432"),
    ("JOBNIAC_DB_NAME", "jobniac_test"),
    ("JOBNIAC_DB_USER", "app_user"),
    ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
];
const CANDIDATE_KEY_PAIR: (&str, &str) = (
    "JOBNIAC_ARBEITNOW_CANDIDATE_KEY",
    "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
);
const CURSOR_SECRET_PAIR: (&str, &str) = (
    "JOBNIAC_CURSOR_HMAC_SECRET",
    "synthetic-cursor-secret-0123456789abcdef",
);
/// The flag of each opt-in group, and the tools it registers in catalog order.
const GROUP_FLAGS: [(&str, &str, &[&str]); 4] = [
    (
        "deletion",
        "JOBNIAC_DELETION_ENABLED",
        &["delete_profile", "delete_job"],
    ),
    ("link", "JOBNIAC_LINK_FETCH_ENABLED", &["add_job_from_link"]),
    (
        "arbeitnow",
        "JOBNIAC_ARBEITNOW_ENABLED",
        &["scan_arbeitnow_jobs", "import_arbeitnow_job"],
    ),
    (
        "artifacts",
        "JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED",
        &["get_document_artifact"],
    ),
];
/// The link group composes only on the platforms its owned fetch child supports.
const LINK_SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "windows"));

/// The environment of the database and key settings, plus the given flag value pairs.
fn environment_with(flags: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut pairs: Vec<(&'static str, &'static str)> = DATABASE_PAIRS.to_vec();
    pairs.push(CANDIDATE_KEY_PAIR);
    pairs.push(CURSOR_SECRET_PAIR);
    pairs.extend_from_slice(flags);
    pairs
}

/// The tool names that `jobniac-cli mcp` lists for exactly `pairs` as its environment.
fn listed_by_binary(pairs: &[(&str, &str)]) -> Result<Vec<String>, Box<dyn Error>> {
    let mut command = cli();
    command.arg("mcp");
    for (key, value) in pairs {
        command.env(key, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or("stdin was not piped")?;
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-11-25","capabilities":{{}},"clientInfo":{{"name":"synthetic-client","version":"0.0.0"}}}}}}"#
        )?;
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )?;
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#)?;
    }
    let output = child.wait_with_output()?;
    if output.status.code() != Some(0) {
        return Err(format!("mcp exited with {:?}", output.status.code()).into());
    }
    let listing = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|message| message["id"] == 2)
        .ok_or("no tools/list response")?;
    Ok(listing["result"]["tools"]
        .as_array()
        .ok_or("tools is not an array")?
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect())
}

/// The tool names of a `tools/list` golden, in order.
fn golden_names(text: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let tools: serde_json::Value = serde_json::from_str(text)?;
    Ok(tools
        .as_array()
        .ok_or("golden is not an array")?
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect())
}

#[test]
fn the_default_binary_lists_the_thirty_four_default_tools_in_catalog_order() -> TestResult {
    let names = listed_by_binary(&[])?;
    let golden = golden_names(include_str!(
        "../../mcp/tests/golden/tools-list-default.json"
    ))?;
    assert_eq!(names, golden);
    assert_eq!(names.len(), 34);
    Ok(())
}

#[test]
fn each_configuration_lists_the_tool_names_in_order() -> TestResult {
    if !LINK_SUPPORTED {
        return Ok(());
    }
    let configurations: [(&str, &[&str], &str); 6] = [
        (
            "deletion",
            &["deletion"],
            include_str!("../../mcp/tests/golden/tools-list-deletion.json"),
        ),
        (
            "link",
            &["link"],
            include_str!("../../mcp/tests/golden/tools-list-link.json"),
        ),
        (
            "arbeitnow",
            &["arbeitnow"],
            include_str!("../../mcp/tests/golden/tools-list-arbeitnow.json"),
        ),
        (
            "artifacts",
            &["artifacts"],
            include_str!("../../mcp/tests/golden/tools-list-artifacts.json"),
        ),
        (
            "full",
            &["deletion", "link", "arbeitnow", "artifacts"],
            include_str!("../../mcp/tests/golden/tools-list-full.json"),
        ),
        (
            "default",
            &[],
            include_str!("../../mcp/tests/golden/tools-list-default.json"),
        ),
    ];
    for (name, groups, golden) in configurations {
        let flags: Vec<(&'static str, &'static str)> = GROUP_FLAGS
            .iter()
            .filter(|(group, _, _)| groups.contains(group))
            .map(|(_, flag, _)| (*flag, "1"))
            .collect();
        let names = listed_by_binary(&environment_with(&flags))?;
        assert_eq!(names, golden_names(golden)?, "configuration {name}");
    }
    Ok(())
}

#[test]
fn only_the_exact_flag_value_composes_each_group() -> TestResult {
    if !LINK_SUPPORTED {
        return Ok(());
    }
    let default = listed_by_binary(&[])?;
    for (group, flag, tools) in GROUP_FLAGS {
        for value in ["0", "true", "01", " 1", "yes", "1 ", ""] {
            let names = listed_by_binary(&environment_with(&[(flag, value)]))?;
            assert_eq!(
                names, default,
                "{group}: flag {value:?} must compose nothing"
            );
        }
        let names = listed_by_binary(&environment_with(&[(flag, "1")]))?;
        for tool in tools {
            assert!(
                names.iter().any(|name| name == tool),
                "{group}: the exact flag must list {tool}"
            );
        }
    }
    Ok(())
}
