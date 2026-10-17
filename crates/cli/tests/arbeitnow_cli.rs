//! The Arbeitnow tools are composed only when `JOBNIAC_ARBEITNOW_ENABLED` is exactly `1`, both keys
//! are valid and a database is configured. The tests drive the built `jobniac-cli` binary over STDIO
//! with a cleared environment. A forged candidate token is refused by the signature check before the
//! repository is reached, so no database connection is made and no test server is needed. The keys
//! are synthetic constants.

use std::error::Error;
use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

const FLAG: &str = "JOBNIAC_ARBEITNOW_ENABLED";
const CANDIDATE_KEY: &str = "JOBNIAC_ARBEITNOW_CANDIDATE_KEY";
const CURSOR_SECRET: &str = "JOBNIAC_CURSOR_HMAC_SECRET";
/// `bytes(range(32))` in unpadded base64url: synthetic, 32 distinct values.
const KEY_VALUE: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
/// A synthetic cursor secret with more than 16 distinct characters.
const SECRET_VALUE: &str = "synthetic-cursor-secret-0123456789abcdef";
/// A token that was never signed by any key.
const FORGED_TOKEN: &str = "Zm9yZ2VkLXN5bnRoZXRpYy1wYXlsb2Fk.c3ludGhldGljLXNpZ25hdHVyZQ";
const DATABASE: [(&str, &str); 5] = [
    ("JOBNIAC_DB_HOST", "127.0.0.1"),
    ("JOBNIAC_DB_PORT", "5432"),
    ("JOBNIAC_DB_NAME", "jobniac_test"),
    ("JOBNIAC_DB_USER", "app_user"),
    ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
];

const SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "windows"));

fn initialize() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic-client","version":"0.0.0"}}}"#
}

/// Run `jobniac-cli mcp` with exactly `pairs` as its environment and `requests` on stdin.
fn run_mcp(pairs: &[(&str, &str)], requests: &[String]) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command.env_clear().arg("mcp");
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
        writeln!(stdin, "{}", initialize())?;
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )?;
        for request in requests {
            writeln!(stdin, "{request}")?;
        }
    }
    Ok(child.wait_with_output()?)
}

fn messages(output: &Output) -> Result<Vec<Value>, Box<dyn Error>> {
    Ok(String::from_utf8(output.stdout.clone())?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?)
}

/// The names `tools/list` reports for the given environment.
fn listed(pairs: &[(&str, &str)]) -> Result<Vec<String>, Box<dyn Error>> {
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_owned();
    let output = run_mcp(pairs, &[list])?;
    assert_eq!(output.status.code(), Some(0));
    let messages = messages(&output)?;
    let tools = messages
        .iter()
        .find(|message| message["id"] == 2)
        .and_then(|message| message["result"]["tools"].as_array())
        .ok_or("tools/list returned no tools")?;
    Ok(tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect())
}

fn enabled(flag: &'static str) -> Vec<(&'static str, &'static str)> {
    let mut pairs = DATABASE.to_vec();
    pairs.push((FLAG, flag));
    pairs.push((CANDIDATE_KEY, KEY_VALUE));
    pairs.push((CURSOR_SECRET, SECRET_VALUE));
    pairs
}

fn has_arbeitnow(names: &[String]) -> (bool, bool) {
    (
        names.iter().any(|name| name == "scan_arbeitnow_jobs"),
        names.iter().any(|name| name == "import_arbeitnow_job"),
    )
}

#[test]
fn the_exact_flag_lists_both_tools_and_the_default_lists_neither() -> TestResult {
    let names = listed(&enabled("1"))?;
    assert_eq!(has_arbeitnow(&names), (SUPPORTED, SUPPORTED));
    assert_eq!(has_arbeitnow(&listed(&DATABASE)?), (false, false));
    Ok(())
}

#[test]
fn any_other_flag_value_lists_neither_tool() -> TestResult {
    for flag in ["0", "true", "01", "", " 1", "yes"] {
        let names = listed(&enabled(flag))?;
        assert_eq!(has_arbeitnow(&names), (false, false), "flag {flag:?}");
    }
    Ok(())
}

#[test]
fn the_flag_without_a_database_lists_neither_tool() -> TestResult {
    let pairs = [
        (FLAG, "1"),
        (CANDIDATE_KEY, KEY_VALUE),
        (CURSOR_SECRET, SECRET_VALUE),
    ];
    if SUPPORTED {
        assert_eq!(has_arbeitnow(&listed(&pairs)?), (false, false));
    }
    Ok(())
}

#[test]
fn an_enabled_flag_with_a_missing_key_stops_startup_without_echoing_settings() -> TestResult {
    let mut pairs = DATABASE.to_vec();
    pairs.push((FLAG, "1"));
    pairs.push((CURSOR_SECRET, SECRET_VALUE));
    let output = run_mcp(&pairs, &[])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(
        stderr.trim_end(),
        "Jobniac could not start. Check local configuration."
    );
    assert!(!stderr.contains(SECRET_VALUE));
    Ok(())
}

#[test]
fn a_forged_candidate_import_is_refused_without_leaking_keys_or_the_token() -> TestResult {
    if !SUPPORTED {
        return Ok(());
    }
    let call = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"import_arbeitnow_job","arguments":{{"request":{{"candidateToken":"{FORGED_TOKEN}"}}}}}}}}"#
    );
    let output = run_mcp(&enabled("1"), &[call])?;
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout.clone())?;
    let stderr = String::from_utf8(output.stderr.clone())?;
    let response = messages(&output)?
        .into_iter()
        .find(|message| message["id"] == 3)
        .ok_or("no tools/call response")?;
    assert_eq!(response["result"]["isError"], true);
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .ok_or("the failure has no text")?;
    let envelope: Value = serde_json::from_str(text)?;
    assert_eq!(envelope["message"], "Invalid Arbeitnow candidate token");
    for secret in [KEY_VALUE, SECRET_VALUE, FORGED_TOKEN] {
        assert!(!stdout.contains(secret), "stdout leaked a secret");
        assert!(!stderr.contains(secret), "stderr leaked a secret");
    }
    Ok(())
}
