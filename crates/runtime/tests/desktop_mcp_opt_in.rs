//! The MCP capabilities that delete data or reach the network are off until the user opts in, and
//! a revoked capability is gone from the running server and from every later start.
//!
//! This is the path the desktop shell takes, without the window: `prepare_environment` builds the
//! effective environment, the supervisor is made from its base pairs and startup grants, and every
//! child is spawned through `McpSupervisor::start` (a recording spawner stands in for the process).
//! The environment the spawner receives is the real environment of the `jobniac-cli http` child; the
//! child composes its tools with the same loaders (`compose_deletion`, `compose_job_links`,
//! `compose_arbeitnow`), so the test composes them over that environment and checks which tools
//! could be registered: `delete_profile` and `delete_job`, `add_job_from_link`, and the Arbeitnow
//! pair `scan_arbeitnow_jobs` and `import_arbeitnow_job` (the only network paths of the catalog).
//! The real tool list of the real binary is checked in `crates/cli/tests/desktop_mcp_tools.rs`.
//! Everything is synthetic: a temporary database and keys directory, no real settings or data.

use std::ffi::OsString;
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jobniac_core::env::Environment;
use jobniac_runtime::app_settings::FeatureSettings;
use jobniac_runtime::arbeitnow::{CANDIDATE_KEY, CURSOR_SECRET_KEY, FLAG_KEY, compose_arbeitnow};
use jobniac_runtime::database::prepare;
use jobniac_runtime::deletion::{DELETION_ENABLED_VARIABLE, compose_deletion};
use jobniac_runtime::desktop_features::{
    DesktopEnvironment, Feature, McpGrants, prepare_environment,
};
use jobniac_runtime::job_links::{LINK_FETCH_ENABLED_VARIABLE, compose_job_links};
use jobniac_runtime::mcp_server::{
    McpChild, McpFailure, McpServerStatus, McpSupervisor, Revocation, SpawnError, SpawnRequest,
    Spawner, Termination,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Pairs = Vec<(OsString, OsString)>;

fn pairs(entries: &[(&str, &str)]) -> Pairs {
    entries
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

/// The tools a composition could register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tools {
    delete_profile: bool,
    delete_job: bool,
    add_job_from_link: bool,
    arbeitnow: bool,
}

const ALL: Tools = Tools::new(true);
const NONE: Tools = Tools::new(false);

impl Tools {
    const fn new(on: bool) -> Self {
        Self {
            delete_profile: on,
            delete_job: on,
            add_job_from_link: on,
            arbeitnow: on,
        }
    }

    const DELETION: Self = Self {
        delete_profile: true,
        delete_job: true,
        ..NONE
    };
    const LINK: Self = Self {
        add_job_from_link: true,
        ..NONE
    };
    const ARBEITNOW: Self = Self {
        arbeitnow: true,
        ..NONE
    };
}

/// A spawner that records the environment of every child and keeps each child's liveness.
#[derive(Default)]
struct Recorder {
    spawned: Mutex<Vec<(Pairs, Arc<AtomicBool>)>>,
    /// While set, a child cannot be confirmed gone when it is terminated.
    stubborn: Arc<AtomicBool>,
}

struct Child {
    alive: Arc<AtomicBool>,
    stubborn: Arc<AtomicBool>,
}

impl McpChild for Child {
    fn is_running(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn terminate(&mut self) -> Termination {
        if self.stubborn.load(Ordering::SeqCst) {
            return Termination::MayBeAlive;
        }
        self.alive.store(false, Ordering::SeqCst);
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
        let alive = Arc::new(AtomicBool::new(true));
        if let Ok(mut spawned) = self.0.spawned.lock() {
            spawned.push((request.environment, Arc::clone(&alive)));
        }
        Ok(Box::new(Child {
            alive,
            stubborn: Arc::clone(&self.0.stubborn),
        }))
    }
}

impl Recorder {
    fn environment(&self, index: usize) -> Option<Pairs> {
        self.spawned
            .lock()
            .ok()?
            .get(index)
            .map(|(environment, _)| environment.clone())
    }

    fn count(&self) -> usize {
        self.spawned.lock().map_or(0, |spawned| spawned.len())
    }

    fn alive(&self, index: usize) -> bool {
        self.spawned.lock().is_ok_and(|spawned| {
            spawned
                .get(index)
                .is_some_and(|(_, alive)| alive.load(Ordering::SeqCst))
        })
    }
}

/// Settings for a run without a generated root: generation is off, the rest is as given.
fn settings(mcp: &[Feature]) -> FeatureSettings {
    let mut features = FeatureSettings::default();
    features.set(Feature::Generation, false);
    for feature in mcp {
        features.set(*feature, true);
    }
    features
}

async fn desktop(
    directory: &Path,
    process: Pairs,
    features: &FeatureSettings,
) -> Result<DesktopEnvironment, Box<dyn std::error::Error>> {
    let database = directory.join("jobniac.sqlite3");
    let database = database.to_str().ok_or("temporary path is not Unicode")?;
    let mut all = pairs(&[("JOBNIAC_DB_PATH", database)]);
    all.extend(process);
    let keys = directory.join("keys");
    Ok(prepare_environment(all, features, Some(&keys)).await)
}

/// The desktop path: the supervisor as the shell builds it, over a recording spawner, and the live
/// listener the start waits for.
struct Server {
    supervisor: McpSupervisor,
    grants: McpGrants,
    recorder: Arc<Recorder>,
    port: u16,
    // Keeps the port open for the readiness check.
    _listener: TcpListener,
}

fn server(prepared: &DesktopEnvironment) -> Result<Server, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let recorder = Arc::new(Recorder::default());
    // The one wiring the shell uses: the base pairs and the grants go in apart.
    let (supervisor, grants) = prepared.supervisor(Arc::new(SharedRecorder(Arc::clone(&recorder))));
    let supervisor = supervisor.with_ready_deadline(Duration::from_secs(2));
    Ok(Server {
        supervisor,
        grants,
        recorder,
        port,
        _listener: listener,
    })
}

impl Server {
    fn start(&self) -> McpServerStatus {
        self.supervisor.start(self.port)
    }

    /// What the settings command does after a switch is turned off: narrow the grants to the
    /// switches that are still on. No value is copied to decide.
    fn revoke_to(&self, features: &FeatureSettings) -> Revocation {
        self.supervisor
            .retain_grants(|pair| self.grants.allows(features, pair))
    }

    /// The tools the child number `index` could register, composed over its real environment.
    fn tools(&self, directory: &Path, index: usize) -> Result<Tools, Box<dyn std::error::Error>> {
        let environment = self.recorder.environment(index).ok_or("no such child")?;
        tools_of(directory, environment)
    }
}

fn tools_of(directory: &Path, environment: Pairs) -> Result<Tools, Box<dyn std::error::Error>> {
    let pool = prepare(&directory.join("jobniac.sqlite3")).ok_or("the database must open")?;
    let env = Environment::from_os_pairs(environment, cfg!(windows));
    let deletion = compose_deletion(&env, Some(&pool));
    Ok(Tools {
        delete_profile: deletion.profile.is_some(),
        delete_job: deletion.job.is_some(),
        add_job_from_link: compose_job_links(&env, Some(&pool))?.is_some(),
        arbeitnow: compose_arbeitnow(&env, Some(&pool))?.is_some(),
    })
}

/// What the app composes from its own environment.
fn app_tools(
    directory: &Path,
    prepared: &DesktopEnvironment,
) -> Result<Tools, Box<dyn std::error::Error>> {
    tools_of(directory, prepared.pairs.clone())
}

/// The tools of the first child of a first start, and of the app.
async fn first_start(
    process: Pairs,
    features: &FeatureSettings,
) -> Result<(Tools, Tools), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let prepared = desktop(dir.path(), process, features).await?;
    let server = server(&prepared)?;
    server.start();
    Ok((
        app_tools(dir.path(), &prepared)?,
        server.tools(dir.path(), 0)?,
    ))
}

#[tokio::test]
async fn a_first_run_keeps_the_in_app_features_and_gives_the_mcp_child_no_such_tool() -> TestResult
{
    let (app, child) = first_start(Vec::new(), &settings(&[])).await?;
    // The app's own deletion, link capture and Arbeitnow are on by default and unchanged.
    assert_eq!(app, ALL);
    // The child has neither the deletion tools, the link tool nor the Arbeitnow tools.
    assert_eq!(child, NONE);
    Ok(())
}

#[tokio::test]
async fn the_default_child_environment_has_no_capability_flag_and_no_arbeitnow_key() -> TestResult {
    let dir = tempfile::tempdir()?;
    let prepared = desktop(dir.path(), Vec::new(), &settings(&[])).await?;
    // The in-app Arbeitnow generated keys for the app.
    assert!(prepared.environment().contains(CANDIDATE_KEY));
    assert!(prepared.environment().contains(CURSOR_SECRET_KEY));
    let server = server(&prepared)?;
    server.start();
    let environment = server.recorder.environment(0).ok_or("no child")?;
    let names: Vec<String> = environment
        .iter()
        .filter_map(|(name, _)| name.to_str().map(str::to_ascii_uppercase))
        .collect();
    for name in [
        DELETION_ENABLED_VARIABLE,
        LINK_FETCH_ENABLED_VARIABLE,
        FLAG_KEY,
        CANDIDATE_KEY,
        CURSOR_SECRET_KEY,
    ] {
        assert!(!names.iter().any(|candidate| candidate == name), "{name}");
    }
    // The child still gets what it needs: the database and the document features.
    assert!(names.iter().any(|name| name == "JOBNIAC_DB_PATH"));
    Ok(())
}

#[tokio::test]
async fn each_opt_in_registers_only_its_own_tools_in_the_child() -> TestResult {
    for (feature, expected) in [
        (Feature::McpDeletion, Tools::DELETION),
        (Feature::McpLinkFetch, Tools::LINK),
        (Feature::McpArbeitnow, Tools::ARBEITNOW),
    ] {
        let (_, child) = first_start(Vec::new(), &settings(&[feature])).await?;
        assert_eq!(child, expected, "{feature:?}");
    }
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    assert_eq!(first_start(Vec::new(), &every).await?.1, ALL);
    Ok(())
}

#[tokio::test]
async fn the_arbeitnow_keys_cross_to_the_child_only_with_its_grant() -> TestResult {
    let dir = tempfile::tempdir()?;
    let every = settings(&[Feature::McpArbeitnow]);
    let prepared = desktop(dir.path(), Vec::new(), &every).await?;
    let server = server(&prepared)?;
    server.start();
    let environment = server.recorder.environment(0).ok_or("no child")?;
    let has = |name: &str| environment.iter().any(|(key, _)| key == name);
    assert!(has(CANDIDATE_KEY) && has(CURSOR_SECRET_KEY) && has(FLAG_KEY));
    // Revoked: the next child has none of the three.
    let mut off = every;
    off.set(Feature::McpArbeitnow, false);
    assert_eq!(server.revoke_to(&off), Revocation::Stopped);
    server.start();
    let environment = server.recorder.environment(1).ok_or("no second child")?;
    let has = |name: &str| environment.iter().any(|(key, _)| key == name);
    assert!(!has(CANDIDATE_KEY) && !has(CURSOR_SECRET_KEY) && !has(FLAG_KEY));
    Ok(())
}

#[tokio::test]
async fn revoking_a_capability_kills_the_running_child_and_the_next_start_lacks_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    let prepared = desktop(dir.path(), Vec::new(), &every).await?;
    let server = server(&prepared)?;
    assert_eq!(server.start(), McpServerStatus::Running);
    assert_eq!(server.tools(dir.path(), 0)?, ALL);

    // The user turns link capture off: the grant is revoked, and the child that held it is dead.
    let mut off = every;
    off.set(Feature::McpLinkFetch, false);
    assert_eq!(server.revoke_to(&off), Revocation::Stopped);
    assert!(!server.recorder.alive(0));
    assert_eq!(server.supervisor.status(), McpServerStatus::Stopped);
    // The restart, a stop and a start, and a start on another port all lack it.
    assert_eq!(server.start(), McpServerStatus::Running);
    server.supervisor.stop();
    assert_eq!(server.start(), McpServerStatus::Running);
    let other = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let other_port = other.local_addr()?.port();
    assert_eq!(
        server.supervisor.start(other_port),
        McpServerStatus::Running
    );
    let expected = Tools {
        add_job_from_link: false,
        ..ALL
    };
    for index in 1..server.recorder.count() {
        assert_eq!(server.tools(dir.path(), index)?, expected, "child {index}");
    }
    // Revoking the rest leaves a child with no such tool.
    let none = settings(&[]);
    assert_eq!(server.revoke_to(&none), Revocation::Stopped);
    server.start();
    assert_eq!(server.tools(dir.path(), server.recorder.count() - 1)?, NONE);
    // A wider allow list cannot give anything back.
    assert_eq!(server.revoke_to(&every), Revocation::Unaffected);
    server.start();
    assert_eq!(server.tools(dir.path(), server.recorder.count() - 1)?, NONE);
    Ok(())
}

#[tokio::test]
async fn the_opt_in_does_not_depend_on_the_in_app_switch() -> TestResult {
    let mut features = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    features.set(Feature::Deletion, false);
    features.set(Feature::LinkFetch, false);
    features.set(Feature::Arbeitnow, false);
    let (app, child) = first_start(Vec::new(), &features).await?;
    assert_eq!(app, NONE);
    assert_eq!(child, ALL);
    Ok(())
}

#[tokio::test]
async fn a_general_flag_of_one_in_the_process_environment_is_no_mcp_opt_in() -> TestResult {
    // `JOBNIAC_*_ENABLED=1` enables the feature for the app, as it always did, and is not a consent
    // to hand the capability to an MCP client.
    let process = [
        (DELETION_ENABLED_VARIABLE, "1"),
        (LINK_FETCH_ENABLED_VARIABLE, "1"),
        (FLAG_KEY, "1"),
    ];
    for features in [settings(&[]), {
        // With the in-app switches off the variables still only enable the app's features.
        let mut off = settings(&[]);
        off.set(Feature::Deletion, false);
        off.set(Feature::LinkFetch, false);
        off.set(Feature::Arbeitnow, false);
        off
    }] {
        let (app, child) = first_start(pairs(&process), &features).await?;
        assert_eq!(app, ALL);
        assert_eq!(child, NONE);
    }
    // One variable at a time.
    for variable in [
        DELETION_ENABLED_VARIABLE,
        LINK_FETCH_ENABLED_VARIABLE,
        FLAG_KEY,
    ] {
        let (_, child) = first_start(pairs(&[(variable, "1")]), &settings(&[])).await?;
        assert_eq!(child, NONE, "{variable}");
    }
    // Only the capability the user opted in to reaches the child, whatever else the process sets.
    let (_, child) = first_start(pairs(&process), &settings(&[Feature::McpLinkFetch])).await?;
    assert_eq!(child, Tools::LINK);
    let (_, child) = first_start(pairs(&process), &settings(&[Feature::McpDeletion])).await?;
    assert_eq!(child, Tools::DELETION);
    Ok(())
}

#[tokio::test]
async fn a_general_flag_of_one_does_not_survive_a_revocation() -> TestResult {
    // The process sets the flag, the user opted in and then revoked: the flag must not bring the
    // capability back at the restart.
    let dir = tempfile::tempdir()?;
    let on = settings(&[Feature::McpDeletion]);
    let prepared = desktop(dir.path(), pairs(&[(DELETION_ENABLED_VARIABLE, "1")]), &on).await?;
    let server = server(&prepared)?;
    server.start();
    assert_eq!(server.tools(dir.path(), 0)?, Tools::DELETION);
    assert_eq!(server.revoke_to(&settings(&[])), Revocation::Stopped);
    server.start();
    assert_eq!(server.tools(dir.path(), 1)?, NONE);
    // The app itself still follows its variable.
    assert!(app_tools(dir.path(), &prepared)?.delete_profile);
    Ok(())
}

#[tokio::test]
async fn a_process_flag_other_than_one_refuses_the_capability_over_the_opt_in() -> TestResult {
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    for value in ["0", "true", "01", " 1", ""] {
        let process = [
            (DELETION_ENABLED_VARIABLE, value),
            (LINK_FETCH_ENABLED_VARIABLE, value),
            (FLAG_KEY, value),
        ];
        let (app, child) = first_start(pairs(&process), &every).await?;
        // The app follows the variables too (they turn its features off).
        assert_eq!(app, NONE, "{value:?}");
        assert_eq!(child, NONE, "{value:?}");
    }
    // A refusal of one capability leaves the others as the user set them.
    let (_, child) = first_start(pairs(&[(DELETION_ENABLED_VARIABLE, "0")]), &every).await?;
    assert_eq!(
        child,
        Tools {
            delete_profile: false,
            delete_job: false,
            ..ALL
        }
    );
    Ok(())
}

#[tokio::test]
async fn names_of_other_case_never_give_the_child_a_capability() -> TestResult {
    // The Windows environment is case-insensitive, so these are the general flags there; on other
    // platforms they are other variables. Either way the child gets no capability from them.
    let process = [
        ("Jobniac_Deletion_Enabled", "1"),
        ("jobniac_link_fetch_enabled", "1"),
        ("JOBNIAC_arbeitnow_ENABLED", "1"),
    ];
    let (_, child) = first_start(pairs(&process), &settings(&[])).await?;
    assert_eq!(child, NONE);
    // With the opt-in the capability comes from the grant, once.
    let (_, child) = first_start(
        pairs(&[("Jobniac_Deletion_Enabled", "1")]),
        &settings(&[Feature::McpDeletion]),
    )
    .await?;
    assert_eq!(child, Tools::DELETION);
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn mixed_case_refusals_win_over_the_opt_in_on_windows() -> TestResult {
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    let process = [
        ("Jobniac_Deletion_Enabled", "0"),
        ("jobniac_link_fetch_enabled", "false"),
        ("JOBNIAC_Arbeitnow_Enabled", ""),
    ];
    let (app, child) = first_start(pairs(&process), &every).await?;
    assert_eq!(app, NONE);
    assert_eq!(child, NONE);
    Ok(())
}

/// A value that starts with `1` and is not Unicode, where the platform can build one.
fn non_unicode_one() -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(OsString::from_vec(vec![0x31, 0xff]))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        Some(OsString::from_wide(&[0x31, 0xD800]))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[tokio::test]
async fn a_non_unicode_flag_value_is_a_refusal_the_child_cannot_read_as_one() -> TestResult {
    let Some(bad) = non_unicode_one() else {
        return Ok(());
    };
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    for variable in [
        DELETION_ENABLED_VARIABLE,
        LINK_FETCH_ENABLED_VARIABLE,
        FLAG_KEY,
    ] {
        let process = vec![(OsString::from(variable), bad.clone())];
        let (_, child) = first_start(process, &every).await?;
        // That capability is off; the other two are as the user set them.
        let expected = match variable {
            DELETION_ENABLED_VARIABLE => Tools {
                delete_profile: false,
                delete_job: false,
                ..ALL
            },
            LINK_FETCH_ENABLED_VARIABLE => Tools {
                add_job_from_link: false,
                ..ALL
            },
            _ => Tools {
                arbeitnow: false,
                ..ALL
            },
        };
        assert_eq!(child, expected, "{variable}");
    }
    Ok(())
}

#[tokio::test]
async fn the_arbeitnow_opt_in_with_unusable_keys_leaves_the_child_without_the_tools() -> TestResult
{
    // A key of the environment that Arbeitnow refuses: the child would fail to start on the flag,
    // so it is not given the flag, and the app stays without Arbeitnow too.
    let mut features = settings(&[Feature::McpArbeitnow]);
    features.set(Feature::Arbeitnow, false);
    let (app, child) =
        first_start(pairs(&[("JOBNIAC_CURSOR_HMAC_SECRET", "short")]), &features).await?;
    assert_eq!(child, NONE);
    assert!(!app.arbeitnow);
    Ok(())
}

#[tokio::test]
async fn a_revocation_that_cannot_stop_the_child_fails_and_no_child_regains_the_capability()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let every = settings(&[
        Feature::McpDeletion,
        Feature::McpLinkFetch,
        Feature::McpArbeitnow,
    ]);
    let prepared = desktop(dir.path(), Vec::new(), &every).await?;
    let server = server(&prepared)?;
    assert_eq!(server.start(), McpServerStatus::Running);

    // The kill cannot be confirmed: the revocation says so, the status is not `stopped`, and the
    // grant is gone for every later start.
    server.recorder.stubborn.store(true, Ordering::SeqCst);
    let mut off = every;
    off.set(Feature::McpDeletion, false);
    assert_eq!(server.revoke_to(&off), Revocation::StopFailed);
    assert_eq!(server.supervisor.status(), McpServerStatus::Failed);
    assert_eq!(server.supervisor.failure(), Some(McpFailure::StopFailed));
    // Nothing is started beside the survivor, so nothing can hold the grant again.
    assert_eq!(server.start(), McpServerStatus::Failed);
    assert_eq!(server.recorder.count(), 1);

    // The survivor is confirmed gone: the next child has everything but deletion.
    server.recorder.stubborn.store(false, Ordering::SeqCst);
    assert_eq!(server.start(), McpServerStatus::Running);
    assert_eq!(
        server.tools(dir.path(), 1)?,
        Tools {
            delete_profile: false,
            delete_job: false,
            ..ALL
        }
    );
    Ok(())
}

#[tokio::test]
async fn the_debug_text_of_the_desktop_pieces_shows_no_secret() -> TestResult {
    let dir = tempfile::tempdir()?;
    let every = settings(&[Feature::McpArbeitnow]);
    let prepared = desktop(dir.path(), Vec::new(), &every).await?;
    let server = server(&prepared)?;
    let candidate = prepared
        .environment()
        .get(CANDIDATE_KEY)
        .ok_or("no candidate key")?
        .to_owned();
    let secret = prepared
        .environment()
        .get(CURSOR_SECRET_KEY)
        .ok_or("no cursor secret")?
        .to_owned();
    for text in [
        format!("{prepared:?}"),
        format!("{:?}", server.grants),
        format!("{:#?}", server.grants),
        format!("{:?}", server.supervisor),
    ] {
        assert!(!text.contains(&candidate), "{text}");
        assert!(!text.contains(&secret), "{text}");
    }
    Ok(())
}
