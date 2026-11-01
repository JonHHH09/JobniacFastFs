//! The real tool list of the real `jobniac-cli` binary, started with the environment that the
//! desktop app's supervisor gives its MCP child.
//!
//! The environment is not hand-written: `prepare_environment` builds the effective environment as
//! the desktop shell does, the supervisor is made from its base pairs and startup grants, and the
//! environment of each child is what `McpSupervisor::start` hands to a recording spawner. The
//! binary is then run in `mcp` mode with exactly that environment (minus the listener-only
//! variables). The `mcp` and `http` modes share `compose_mcp_services`, so the tool list is the one
//! the HTTP child serves. The binary starts with a cleared environment, over a temporary database,
//! and nothing real is read or written.

use std::error::Error;
use std::ffi::OsString;
use std::io::Write;
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jobniac_runtime::app_settings::FeatureSettings;
use jobniac_runtime::database::prepare;
use jobniac_runtime::desktop_features::{
    DesktopEnvironment, Feature, McpGrants, prepare_environment,
};
use jobniac_runtime::mcp_server::{
    McpChild, McpServerStatus, McpSupervisor, Revocation, SpawnError, SpawnRequest, Spawner,
    Termination,
};

type TestResult = Result<(), Box<dyn Error>>;
type Pairs = Vec<(OsString, OsString)>;

const DELETION: [&str; 2] = ["delete_profile", "delete_job"];
const LINK: [&str; 1] = ["add_job_from_link"];
const ARBEITNOW: [&str; 2] = ["scan_arbeitnow_jobs", "import_arbeitnow_job"];

/// The tools that are listed with no capability flag: 34 plus `get_document_artifact`, which the
/// desktop defaults turn on.
const DEFAULT_COUNT: usize = 35;

#[derive(Default)]
struct Recorder {
    environments: Mutex<Vec<Pairs>>,
}

struct Child(Arc<AtomicBool>);

impl McpChild for Child {
    fn is_running(&mut self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn terminate(&mut self) -> Termination {
        self.0.store(false, Ordering::SeqCst);
        Termination::Gone
    }
}

struct SharedRecorder(Arc<Recorder>);

impl Spawner for SharedRecorder {
    fn available(&self) -> bool {
        true
    }

    fn port_free(&self, _port: u16) -> bool {
        true
    }

    fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError> {
        if let Ok(mut environments) = self.0.environments.lock() {
            environments.push(request.environment);
        }
        Ok(Box::new(Child(Arc::new(AtomicBool::new(true)))))
    }
}

fn pairs(entries: &[(&str, &str)]) -> Pairs {
    entries
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

fn settings(mcp: &[Feature]) -> FeatureSettings {
    let mut features = FeatureSettings::default();
    // No generated root: the run registers nothing on the machine.
    features.set(Feature::Generation, false);
    for feature in mcp {
        features.set(*feature, true);
    }
    features
}

fn desktop(
    directory: &Path,
    process: Pairs,
    features: &FeatureSettings,
) -> Result<DesktopEnvironment, Box<dyn Error>> {
    let database = directory.join("jobniac.sqlite3");
    let database = database.to_str().ok_or("temporary path is not Unicode")?;
    let mut all = pairs(&[("JOBNIAC_DB_PATH", database)]);
    all.extend(process);
    let keys = directory.join("keys");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let prepared = runtime.block_on(prepare_environment(all, features, Some(&keys)));
    // The child opens the database as it is: create and migrate it first, as the app does.
    prepare(&database_file(directory)).ok_or("the database must open")?;
    Ok(prepared)
}

fn database_file(directory: &Path) -> std::path::PathBuf {
    directory.join("jobniac.sqlite3")
}

/// The supervisor as the desktop shell builds it, over a recording spawner.
struct Desktop {
    supervisor: McpSupervisor,
    grants: McpGrants,
    recorder: Arc<Recorder>,
    port: u16,
    _listener: TcpListener,
}

fn desktop_server(prepared: DesktopEnvironment) -> Result<Desktop, Box<dyn Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let recorder = Arc::new(Recorder::default());
    // The one wiring the shell uses: the base pairs and the grants go in apart.
    let (supervisor, grants) = prepared.supervisor(Arc::new(SharedRecorder(Arc::clone(&recorder))));
    let supervisor = supervisor.with_ready_deadline(Duration::from_secs(2));
    Ok(Desktop {
        supervisor,
        grants,
        recorder,
        port,
        _listener: listener,
    })
}

impl Desktop {
    fn start(&self) -> McpServerStatus {
        self.supervisor.start(self.port)
    }

    /// Revokes everything the switches of `features` no longer allow.
    fn revoke_to(&self, features: &FeatureSettings) -> bool {
        self.supervisor
            .retain_grants(|pair| self.grants.allows(features, pair))
            == Revocation::Stopped
    }

    /// The tools the real binary lists with the environment of the child number `index`.
    fn tools(&self, index: usize) -> Result<Vec<String>, Box<dyn Error>> {
        let environment = self
            .recorder
            .environments
            .lock()
            .map_err(|_| "poisoned")?
            .get(index)
            .cloned()
            .ok_or("no such child")?;
        list_tools(environment)
    }

    fn last_tools(&self) -> Result<Vec<String>, Box<dyn Error>> {
        let count = self
            .recorder
            .environments
            .lock()
            .map_err(|_| "poisoned")?
            .len();
        self.tools(count.checked_sub(1).ok_or("no child was started")?)
    }
}

/// Runs `jobniac-cli mcp` with exactly `environment` (the listener-only variables removed, as the
/// stdio mode has no listener) and returns the names of the tools it lists.
fn list_tools(environment: Pairs) -> Result<Vec<String>, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command.env_clear();
    for (name, value) in environment {
        let upper = name.to_string_lossy().to_uppercase();
        if upper.starts_with("JOBNIAC_HTTP_") || upper == "JOBNIAC_INTERNAL_PARENT_PIPE" {
            continue;
        }
        command.env(name, value);
    }
    let mut child = command
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or("stdin was not piped")?;
        for request in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic-client","version":"0.0.0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ] {
            writeln!(stdin, "{request}")?;
        }
    }
    let output = child.wait_with_output()?;
    if output.status.code() != Some(0) {
        return Err(format!("the binary exited with {:?}", output.status.code()).into());
    }
    let messages: Vec<serde_json::Value> = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let listing = messages.get(1).ok_or("no tools/list answer")?;
    Ok(listing["result"]["tools"]
        .as_array()
        .ok_or("tools is not an array")?
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect())
}

fn has_all(tools: &[String], names: &[&str]) -> bool {
    names
        .iter()
        .all(|name| tools.iter().any(|tool| tool == name))
}

fn has_any(tools: &[String], names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| tools.iter().any(|tool| tool == name))
}

fn gated() -> Vec<&'static str> {
    DELETION
        .iter()
        .chain(&LINK)
        .chain(&ARBEITNOW)
        .copied()
        .collect()
}

fn start(
    process: Pairs,
    features: &FeatureSettings,
) -> Result<(tempfile::TempDir, Desktop), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let prepared = desktop(dir.path(), process, features)?;
    let server = desktop_server(prepared)?;
    assert_eq!(server.start(), McpServerStatus::Running);
    Ok((dir, server))
}

#[test]
fn the_default_desktop_child_lists_no_network_or_deletion_tool() -> TestResult {
    let (_dir, server) = start(Vec::new(), &settings(&[]))?;
    let tools = server.tools(0)?;
    assert!(!has_any(&tools, &gated()), "{tools:?}");
    assert_eq!(tools.len(), DEFAULT_COUNT, "{tools:?}");
    assert!(has_all(
        &tools,
        &["get_document_artifact", "health", "add_job_from_text"]
    ));
    Ok(())
}

#[test]
fn each_opt_in_lists_exactly_its_own_tools() -> TestResult {
    for (feature, names) in [
        (Feature::McpDeletion, DELETION.to_vec()),
        (Feature::McpLinkFetch, LINK.to_vec()),
        (Feature::McpArbeitnow, ARBEITNOW.to_vec()),
    ] {
        let (_dir, server) = start(Vec::new(), &settings(&[feature]))?;
        let tools = server.tools(0)?;
        assert!(has_all(&tools, &names), "{feature:?}: {tools:?}");
        assert_eq!(tools.len(), DEFAULT_COUNT + names.len(), "{feature:?}");
        let others: Vec<&str> = gated()
            .into_iter()
            .filter(|name| !names.contains(name))
            .collect();
        assert!(!has_any(&tools, &others), "{feature:?}: {tools:?}");
    }
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    let (_dir, server) = start(Vec::new(), &every)?;
    let tools = server.tools(0)?;
    assert_eq!(tools.len(), 40, "{tools:?}");
    assert!(has_all(&tools, &gated()));
    Ok(())
}

#[test]
fn a_general_flag_of_one_in_the_process_environment_lists_no_such_tool() -> TestResult {
    let process = pairs(&[
        ("JOBNIAC_DELETION_ENABLED", "1"),
        ("JOBNIAC_LINK_FETCH_ENABLED", "1"),
        ("JOBNIAC_ARBEITNOW_ENABLED", "1"),
    ]);
    let (_dir, server) = start(process, &settings(&[]))?;
    let tools = server.tools(0)?;
    assert!(!has_any(&tools, &gated()), "{tools:?}");
    Ok(())
}

#[test]
fn a_process_flag_other_than_one_removes_the_tools_over_an_opt_in() -> TestResult {
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    let process = pairs(&[
        ("JOBNIAC_DELETION_ENABLED", "0"),
        ("JOBNIAC_LINK_FETCH_ENABLED", "true"),
        ("JOBNIAC_ARBEITNOW_ENABLED", ""),
    ]);
    let (_dir, server) = start(process, &every)?;
    let tools = server.tools(0)?;
    assert!(!has_any(&tools, &gated()), "{tools:?}");
    Ok(())
}

#[test]
fn a_revoked_capability_is_gone_from_every_later_child_of_the_desktop_path() -> TestResult {
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    let (_dir, server) = start(Vec::new(), &every)?;
    assert!(has_all(&server.tools(0)?, &gated()));

    // Link capture off: the running child is stopped, and the restart lists everything else.
    let mut off = every;
    off.set(Feature::McpLinkFetch, false);
    assert!(server.revoke_to(&off));
    assert_eq!(server.start(), McpServerStatus::Running);
    let tools = server.last_tools()?;
    assert!(!has_any(&tools, &LINK), "{tools:?}");
    assert!(has_all(&tools, &DELETION) && has_all(&tools, &ARBEITNOW));

    // A stop and a start does not bring it back.
    server.supervisor.stop();
    assert_eq!(server.start(), McpServerStatus::Running);
    assert!(!has_any(&server.last_tools()?, &LINK));

    // Everything off: back to the default list.
    assert!(server.revoke_to(&settings(&[])));
    assert_eq!(server.start(), McpServerStatus::Running);
    let tools = server.last_tools()?;
    assert!(!has_any(&tools, &gated()), "{tools:?}");
    assert_eq!(tools.len(), DEFAULT_COUNT);

    // A wider allow list changes nothing: a revoked grant does not come back in this run.
    assert!(!server.revoke_to(&every));
    assert_eq!(server.start(), McpServerStatus::Running);
    assert!(!has_any(&server.last_tools()?, &gated()));
    Ok(())
}
