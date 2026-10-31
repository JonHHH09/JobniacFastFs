//! Debug-only isolation of a test run from the per-user folders of the machine.
//!
//! Tauri resolves the app config, data, local data, cache and log directories through the Known
//! Folder APIs on Windows (`APPDATA` and `LOCALAPPDATA` are ignored), and the WebView2 profile
//! (the browser session) defaults to the app local data directory. A test run therefore needs the
//! directories redirected inside the process, before the application is built.
//!
//! The gate is explicit and fails closed:
//!
//! - this module only exists in builds with `debug_assertions` (`cfg(debug_assertions)` on the
//!   `mod` line and on every use in `lib.rs`), so a release build has no code and no variable name
//!   for it, and ignores `JOBNIAC_TEST_RUN_ROOT` entirely;
//! - the isolation is on only when `JOBNIAC_TEST_RUN_ROOT` is present. A present value that is not
//!   a clean absolute path, a root that was not prepared by the harness (it must hold the marker
//!   file), a database, generated root or temporary directory outside the root, or a resolved
//!   directory that is not the planned one ends the start with a fixed message. Nothing falls back
//!   to the real folders.
//!
//! Every message is fixed text: none names a path, a variable value or a setting.

use std::fs;
use std::path::{Path, PathBuf};

use jobniac_core::database_location::DB_PATH_VARIABLE;
use jobniac_core::env::{EnvValue, Environment};
use jobniac_core::generated_root::{RootPlatform, admit_root_path};
use tauri::Manager;
use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides, Config};

/// The variable that turns the isolation on: the absolute path of the prepared run root.
pub const ROOT_VARIABLE: &str = "JOBNIAC_TEST_RUN_ROOT";
/// The marker file the harness writes into a run root. Without it the root is refused.
pub const MARKER_FILE: &str = ".jobniac-test-run";
/// The exact content of the marker file.
pub const MARKER_TEXT: &str = "jobniac-test-run-v1\n";
/// The report the application writes into the run root once the isolation is verified.
pub const REPORT_FILE: &str = "isolation-report.json";

/// The directory of each app directory under the run root.
pub const CONFIG_DIR: &str = "app-config";
/// See [`CONFIG_DIR`].
pub const DATA_DIR: &str = "app-data";
/// See [`CONFIG_DIR`]. The WebView2 profile lives here (`EBWebView`).
pub const LOCAL_DATA_DIR: &str = "app-local-data";
/// See [`CONFIG_DIR`].
pub const CACHE_DIR: &str = "app-cache";
/// See [`CONFIG_DIR`].
pub const LOG_DIR: &str = "app-log";

const GENERATED_ROOT_VARIABLE: &str = "JOBNIAC_GENERATED_ROOT";
const IMPORT_ROOT_VARIABLE: &str = "JOBNIAC_DOCUMENT_IMPORT_ROOT";
const WEBVIEW_PROFILE_VARIABLE: &str = "WEBVIEW2_USER_DATA_FOLDER";

/// Why the isolation refused the start. The text names no path or value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationError {
    /// The run root variable is not a clean absolute path.
    BadRoot,
    /// The run root does not exist, is not a directory, or does not hold the marker.
    UnpreparedRoot,
    /// A required variable is missing, or a path variable lies outside the run root.
    PathOutsideRoot,
    /// A directory Tauri resolved is not the planned one under the run root.
    DirectoryNotRedirected,
    /// The run root is, contains or lies inside a real per-user folder of the machine.
    RootOverlapsRealFolders,
    /// A planned directory could not be created, or resolves outside the run root.
    DirectoryEscapesRoot,
    /// The report could not be written.
    ReportNotWritten,
}

impl IsolationError {
    /// The fixed startup text.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::BadRoot => "test isolation: the run root is not a clean absolute path",
            Self::UnpreparedRoot => "test isolation: the run root is not a prepared run folder",
            Self::PathOutsideRoot => {
                "test isolation: a required path is missing or lies outside the run root"
            }
            Self::DirectoryNotRedirected => {
                "test isolation: an app directory was not redirected into the run root"
            }
            Self::RootOverlapsRealFolders => {
                "test isolation: the run root overlaps a real per-user folder"
            }
            Self::DirectoryEscapesRoot => {
                "test isolation: an app directory could not be confined to the run root"
            }
            Self::ReportNotWritten => "test isolation: the isolation report could not be written",
        }
    }
}

/// A verified-by-construction layout: the five app directories under one clean absolute root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    root: PathBuf,
    database: PathBuf,
    generated_root: PathBuf,
    /// The temporary directories and the optional import root and WebView2 profile override: they
    /// get the same link-resolving check as the database and the generated root.
    also_confined: Vec<PathBuf>,
}

/// The directories Tauri resolved, in the order of [`Plan::expected`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// `app_config_dir` (settings.json).
    pub config: PathBuf,
    /// `app_data_dir`.
    pub data: PathBuf,
    /// `app_local_data_dir` (feature-keys.json and the WebView2 profile).
    pub local_data: PathBuf,
    /// `app_cache_dir`.
    pub cache: PathBuf,
    /// `app_log_dir`.
    pub log: PathBuf,
}

fn platform() -> RootPlatform {
    if cfg!(windows) {
        RootPlatform::Windows
    } else {
        RootPlatform::Other
    }
}

/// Windows paths compare case-insensitively, but `Path` compares components exactly (only the
/// drive letter is folded), so the overlap check compares lower-cased text there.
fn fold_text(text: &str) -> String {
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_owned()
    }
}

fn folded(path: &Path) -> PathBuf {
    PathBuf::from(fold_text(&path.to_string_lossy()))
}

/// A clean absolute path (the rules of `JOBNIAC_DB_PATH`) that lies strictly inside `root`,
/// compared by whole path components.
fn strictly_inside(root: &Path, value: &str) -> Option<PathBuf> {
    let admitted = admit_root_path(value, platform())?;
    let path = PathBuf::from(admitted);
    (path != root && path.starts_with(root)).then_some(path)
}

/// The path variable `name`: `Ok(None)` when missing, the path when it lies inside `root`, and an
/// error when it is present but invalid or outside.
fn confined(
    environment: &Environment,
    root: &Path,
    name: &str,
) -> Result<Option<PathBuf>, IsolationError> {
    match environment.lookup(name) {
        EnvValue::Missing => Ok(None),
        EnvValue::Invalid => Err(IsolationError::PathOutsideRoot),
        EnvValue::Valid(value) => strictly_inside(root, value)
            .map(Some)
            .ok_or(IsolationError::PathOutsideRoot),
    }
}

fn required(environment: &Environment, root: &Path, name: &str) -> Result<PathBuf, IsolationError> {
    confined(environment, root, name)?.ok_or(IsolationError::PathOutsideRoot)
}

/// Reads the isolation request from the process environment: `Ok(None)` when the run root
/// variable is absent (no isolation), the plan when it is present and everything is confined.
///
/// The database file, the generated root and the temporary directory (`TMP` and `TEMP` on Windows,
/// `TMPDIR` elsewhere) must be named and lie inside the run root: the app never reaches its
/// per-user defaults for them. The import root and the WebView2 profile override are optional and
/// are checked when present.
///
/// # Errors
///
/// An [`IsolationError`] for a present but unusable root or an unconfined path.
pub fn plan(environment: &Environment) -> Result<Option<Plan>, IsolationError> {
    let root = match environment.lookup(ROOT_VARIABLE) {
        EnvValue::Missing => return Ok(None),
        EnvValue::Invalid => return Err(IsolationError::BadRoot),
        EnvValue::Valid(value) => {
            PathBuf::from(admit_root_path(value, platform()).ok_or(IsolationError::BadRoot)?)
        }
    };
    let database = required(environment, &root, DB_PATH_VARIABLE)?;
    let generated_root = required(environment, &root, GENERATED_ROOT_VARIABLE)?;
    let temporary: &[&str] = if cfg!(windows) {
        &["TMP", "TEMP"]
    } else {
        &["TMPDIR"]
    };
    let mut also_confined = Vec::new();
    for name in temporary {
        also_confined.push(required(environment, &root, name)?);
    }
    for name in [IMPORT_ROOT_VARIABLE, WEBVIEW_PROFILE_VARIABLE] {
        also_confined.extend(confined(environment, &root, name)?);
    }
    Ok(Some(Plan {
        root,
        database,
        generated_root,
        also_confined,
    }))
}

impl Plan {
    /// The planned app directories.
    #[must_use]
    pub fn expected(&self) -> Resolved {
        Resolved {
            config: self.root.join(CONFIG_DIR),
            data: self.root.join(DATA_DIR),
            local_data: self.root.join(LOCAL_DATA_DIR),
            cache: self.root.join(CACHE_DIR),
            log: self.root.join(LOG_DIR),
        }
    }

    /// Redirects all five app directories of `config`. Applied before the application is built.
    pub fn apply(&self, config: &mut Config) {
        let expected = self.expected();
        config.app.app_directories_override =
            Some(AppDirectoriesOverride::Directories(AppDirectoryOverrides {
                config: Some(expected.config),
                data: Some(expected.data),
                local_data: Some(expected.local_data),
                cache: Some(expected.cache),
                log: Some(expected.log),
            }));
    }

    /// Checks the built application: the run root is a prepared folder, Tauri resolves exactly the
    /// planned directories, none of the real per-user folders overlaps the run root, and every
    /// planned path stays inside the root after links are resolved. Writes the report on success.
    /// Runs before any window exists.
    ///
    /// # Errors
    ///
    /// The first [`IsolationError`] found.
    pub fn verify<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> Result<(), IsolationError> {
        self.verify_root_prepared()?;
        let path = app.path();
        let resolved = Resolved {
            config: path
                .app_config_dir()
                .map_err(|_| IsolationError::DirectoryNotRedirected)?,
            data: path
                .app_data_dir()
                .map_err(|_| IsolationError::DirectoryNotRedirected)?,
            local_data: path
                .app_local_data_dir()
                .map_err(|_| IsolationError::DirectoryNotRedirected)?,
            cache: path
                .app_cache_dir()
                .map_err(|_| IsolationError::DirectoryNotRedirected)?,
            log: path
                .app_log_dir()
                .map_err(|_| IsolationError::DirectoryNotRedirected)?,
        };
        self.verify_resolved(&resolved)?;
        // The real folders: the base directories are not touched by the override.
        let identifier = &app.config().identifier;
        let bases: Vec<PathBuf> = [
            path.config_dir(),
            path.data_dir(),
            path.local_data_dir(),
            path.cache_dir(),
            path.home_dir(),
        ]
        .into_iter()
        .flatten()
        .collect();
        self.verify_no_overlap(&bases, identifier)?;
        self.verify_confined(&resolved)?;
        self.write_report(&resolved)
    }

    /// The run root exists, is a directory and holds the marker with its exact text.
    fn verify_root_prepared(&self) -> Result<(), IsolationError> {
        let is_directory = fs::symlink_metadata(&self.root).is_ok_and(|meta| meta.is_dir());
        let marked =
            fs::read_to_string(self.root.join(MARKER_FILE)).is_ok_and(|text| text == MARKER_TEXT);
        if is_directory && marked {
            Ok(())
        } else {
            Err(IsolationError::UnpreparedRoot)
        }
    }

    /// Tauri resolved exactly the planned directories (not merely some directory inside the root).
    fn verify_resolved(&self, resolved: &Resolved) -> Result<(), IsolationError> {
        // The log directory is resolved under a `Directories` override as given, no subfolder.
        if *resolved == self.expected() {
            Ok(())
        } else {
            Err(IsolationError::DirectoryNotRedirected)
        }
    }

    /// The root is not a real base folder or an ancestor of one, and is not a real app directory
    /// (`<base>/<identifier>`) or inside one.
    fn verify_no_overlap(&self, bases: &[PathBuf], identifier: &str) -> Result<(), IsolationError> {
        let root = folded(&self.root);
        for base in bases {
            let base = folded(base);
            let real_app_directory = base.join(fold_text(identifier));
            if root == base || base.starts_with(&root) || root.starts_with(&real_app_directory) {
                return Err(IsolationError::RootOverlapsRealFolders);
            }
        }
        Ok(())
    }

    /// Creates the planned directories and checks, after links and junctions are resolved, that each
    /// lies inside the resolved root. The database, generated root and their existing ancestors
    /// are checked the same way.
    fn verify_confined(&self, resolved: &Resolved) -> Result<(), IsolationError> {
        let root =
            fs::canonicalize(&self.root).map_err(|_| IsolationError::DirectoryEscapesRoot)?;
        for directory in [
            &resolved.config,
            &resolved.data,
            &resolved.local_data,
            &resolved.cache,
            &resolved.log,
        ] {
            fs::create_dir_all(directory).map_err(|_| IsolationError::DirectoryEscapesRoot)?;
            let real =
                fs::canonicalize(directory).map_err(|_| IsolationError::DirectoryEscapesRoot)?;
            if real == root || !real.starts_with(&root) {
                return Err(IsolationError::DirectoryEscapesRoot);
            }
        }
        let planned = [&self.database, &self.generated_root]
            .into_iter()
            .chain(&self.also_confined);
        for path in planned {
            // The file or folder may not exist yet: its nearest existing ancestor decides.
            let existing = path
                .ancestors()
                .find(|ancestor| ancestor.exists())
                .ok_or(IsolationError::DirectoryEscapesRoot)?;
            let real =
                fs::canonicalize(existing).map_err(|_| IsolationError::DirectoryEscapesRoot)?;
            if !real.starts_with(&root) {
                return Err(IsolationError::DirectoryEscapesRoot);
            }
        }
        Ok(())
    }

    /// The evidence the harness reads after the run: what Tauri resolved and what the environment
    /// pinned. Debug-run data only (synthetic folders), never shown in the app.
    fn write_report(&self, resolved: &Resolved) -> Result<(), IsolationError> {
        let report = serde_json::json!({
            "verified": true,
            "root": self.root,
            "appConfigDir": resolved.config,
            "appDataDir": resolved.data,
            "appLocalDataDir": resolved.local_data,
            "appCacheDir": resolved.cache,
            "appLogDir": resolved.log,
            "database": self.database,
            "generatedRoot": self.generated_root,
        });
        let text =
            serde_json::to_string_pretty(&report).map_err(|_| IsolationError::ReportNotWritten)?;
        fs::write(self.root.join(REPORT_FILE), text).map_err(|_| IsolationError::ReportNotWritten)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A synthetic drive-absolute root on Windows, an absolute one elsewhere.
    fn root_text(name: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\jobniac-test\{name}")
        } else {
            format!("/jobniac-test/{name}")
        }
    }

    fn join(base: &str, leaf: &str) -> String {
        format!("{base}{}{leaf}", if cfg!(windows) { '\\' } else { '/' })
    }

    /// A complete, confined environment for `root`.
    fn pairs(root: &str) -> Vec<(String, String)> {
        let mut pairs = vec![
            (ROOT_VARIABLE.to_owned(), root.to_owned()),
            (DB_PATH_VARIABLE.to_owned(), join(root, "jobniac.sqlite3")),
            (GENERATED_ROOT_VARIABLE.to_owned(), join(root, "generated")),
        ];
        let temporary: &[&str] = if cfg!(windows) {
            &["TMP", "TEMP"]
        } else {
            &["TMPDIR"]
        };
        for name in temporary {
            pairs.push(((*name).to_owned(), join(root, "tmp")));
        }
        pairs
    }

    fn environment(pairs: Vec<(String, String)>) -> Environment {
        Environment::from_os_pairs(
            pairs
                .into_iter()
                .map(|(name, value)| (name.into(), value.into())),
            cfg!(windows),
        )
    }

    fn without(mut pairs: Vec<(String, String)>, name: &str) -> Vec<(String, String)> {
        pairs.retain(|(key, _)| key != name);
        pairs
    }

    fn with(mut pairs: Vec<(String, String)>, name: &str, value: &str) -> Vec<(String, String)> {
        pairs.retain(|(key, _)| key != name);
        pairs.push((name.to_owned(), value.to_owned()));
        pairs
    }

    #[test]
    fn without_the_run_root_variable_the_isolation_is_off() {
        assert_eq!(plan(&Environment::default()), Ok(None));
        let others = without(pairs(&root_text("run")), ROOT_VARIABLE);
        assert_eq!(plan(&environment(others)), Ok(None));
    }

    #[test]
    fn a_complete_environment_gives_a_plan_with_five_distinct_directories_under_the_root() {
        let root = root_text("run");
        let plan = plan(&environment(pairs(&root)))
            .ok()
            .flatten()
            .expect("a plan");
        assert_eq!(plan.root, Path::new(&root));
        let expected = plan.expected();
        let all = [
            &expected.config,
            &expected.data,
            &expected.local_data,
            &expected.cache,
            &expected.log,
        ];
        for (index, directory) in all.iter().enumerate() {
            assert!(directory.starts_with(&root), "{directory:?}");
            assert_ne!(**directory, Path::new(&root), "{directory:?}");
            for other in &all[index + 1..] {
                assert_ne!(directory, other);
            }
        }
    }

    #[test]
    fn a_present_but_unusable_root_fails_closed() {
        let relative = "relative/run";
        let dotted = join(&root_text("run"), "..");
        let padded = format!(" {} ", root_text("run"));
        for value in [
            "",
            relative,
            dotted.as_str(),
            padded.as_str(),
            r"\\server\share",
        ] {
            let env = environment(with(pairs(&root_text("run")), ROOT_VARIABLE, value));
            assert_eq!(plan(&env), Err(IsolationError::BadRoot), "{value:?}");
        }
        let invalid = Environment::default().with_invalid(ROOT_VARIABLE);
        assert_eq!(plan(&invalid), Err(IsolationError::BadRoot));
    }

    #[test]
    fn the_database_the_generated_root_and_the_temporary_directory_are_required() {
        let root = root_text("run");
        let mut names = vec![DB_PATH_VARIABLE, GENERATED_ROOT_VARIABLE];
        names.extend(if cfg!(windows) {
            vec!["TMP", "TEMP"]
        } else {
            vec!["TMPDIR"]
        });
        for name in names {
            let env = environment(without(pairs(&root), name));
            assert_eq!(plan(&env), Err(IsolationError::PathOutsideRoot), "{name}");
        }
    }

    #[test]
    fn a_path_outside_the_root_or_sharing_only_its_prefix_is_refused() {
        let root = root_text("run");
        let sibling = join(&root_text("run-other"), "jobniac.sqlite3");
        let elsewhere = if cfg!(windows) {
            r"C:\Users\someone\AppData\Local\Jobniac\data\jobniac.sqlite3".to_owned()
        } else {
            "/home/someone/.local/share/jobniac/data/jobniac.sqlite3".to_owned()
        };
        for value in [
            sibling,
            elsewhere,
            root.clone(),
            "relative.sqlite3".to_owned(),
        ] {
            for name in [DB_PATH_VARIABLE, GENERATED_ROOT_VARIABLE] {
                let env = environment(with(pairs(&root), name, &value));
                assert_eq!(
                    plan(&env),
                    Err(IsolationError::PathOutsideRoot),
                    "{name} {value:?}"
                );
            }
        }
    }

    #[test]
    fn the_optional_paths_are_checked_when_present() {
        let root = root_text("run");
        let outside = join(&root_text("run-other"), "x");
        for name in [IMPORT_ROOT_VARIABLE, WEBVIEW_PROFILE_VARIABLE] {
            let inside = environment(with(pairs(&root), name, &join(&root, "optional")));
            assert!(matches!(plan(&inside), Ok(Some(_))), "{name}");
            let escaped = environment(with(pairs(&root), name, &outside));
            assert_eq!(
                plan(&escaped),
                Err(IsolationError::PathOutsideRoot),
                "{name}"
            );
        }
    }

    fn some_plan(root: &Path) -> Plan {
        let text = root.to_string_lossy().into_owned();
        plan(&environment(pairs(&text)))
            .ok()
            .flatten()
            .expect("a plan")
    }

    #[test]
    fn apply_redirects_all_five_directories() {
        let plan = some_plan(Path::new(&root_text("run")));
        let mut context: tauri::Context<tauri::test::MockRuntime> =
            tauri::test::mock_context(tauri::test::noop_assets());
        plan.apply(context.config_mut());
        let expected = plan.expected();
        assert_eq!(
            context.config().app.app_directories_override,
            Some(AppDirectoriesOverride::Directories(AppDirectoryOverrides {
                config: Some(expected.config),
                data: Some(expected.data),
                local_data: Some(expected.local_data),
                cache: Some(expected.cache),
                log: Some(expected.log),
            }))
        );
    }

    #[test]
    fn resolved_directories_must_be_exactly_the_planned_ones() {
        let plan = some_plan(Path::new(&root_text("run")));
        assert_eq!(plan.verify_resolved(&plan.expected()), Ok(()));
        let real = |name: &str| PathBuf::from(root_text("real")).join(name);
        let fields: [fn(&mut Resolved, PathBuf); 5] = [
            |resolved, path| resolved.config = path,
            |resolved, path| resolved.data = path,
            |resolved, path| resolved.local_data = path,
            |resolved, path| resolved.cache = path,
            |resolved, path| resolved.log = path,
        ];
        for set in fields {
            let mut resolved = plan.expected();
            set(&mut resolved, real("somewhere"));
            assert_eq!(
                plan.verify_resolved(&resolved),
                Err(IsolationError::DirectoryNotRedirected)
            );
        }
        // Inside the root but not the planned directory is refused too.
        let mut resolved = plan.expected();
        resolved.config = plan.root.join("other");
        assert_eq!(
            plan.verify_resolved(&resolved),
            Err(IsolationError::DirectoryNotRedirected)
        );
    }

    #[test]
    fn a_root_overlapping_a_real_folder_is_refused() {
        let identifier = "io.example.app";
        let base = PathBuf::from(root_text("real-base"));
        let plan_at = |root: PathBuf| some_plan(&root);
        // Unrelated root: fine.
        assert_eq!(
            plan_at(PathBuf::from(root_text("run")))
                .verify_no_overlap(std::slice::from_ref(&base), identifier),
            Ok(())
        );
        // A root inside the real base (but not the real app directory) is fine: the run folder may
        // live under the home folder.
        assert_eq!(
            plan_at(base.join("project").join("run"))
                .verify_no_overlap(std::slice::from_ref(&base), identifier),
            Ok(())
        );
        for root in [
            base.clone(),                                             // the base itself
            base.parent().map(Path::to_path_buf).unwrap_or_default(), // an ancestor of the base
            base.join(identifier),                                    // the real app directory
            base.join(identifier).join("run"), // inside the real app directory
        ] {
            assert_eq!(
                plan_at(root.clone()).verify_no_overlap(std::slice::from_ref(&base), identifier),
                Err(IsolationError::RootOverlapsRealFolders),
                "{root:?}"
            );
        }
    }

    /// Windows folds case: the same real folder spelled in other case is still that folder.
    #[cfg(windows)]
    #[test]
    fn the_overlap_check_ignores_case_on_windows() {
        let identifier = "io.example.app";
        let base = PathBuf::from(root_text("Real-Base"));
        let shouted = PathBuf::from(root_text("REAL-BASE"));
        for root in [
            shouted.clone(),
            shouted.join(identifier.to_uppercase()).join("run"),
        ] {
            assert_eq!(
                some_plan(&root).verify_no_overlap(std::slice::from_ref(&base), identifier),
                Err(IsolationError::RootOverlapsRealFolders),
                "{root:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_temporary_directory_that_is_a_junction_out_of_the_root_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let root = dir.path().join("run");
        fs::create_dir(&root)?;
        let tmp = root.join("tmp");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&tmp)
            .arg(outside.path())
            .stdout(std::process::Stdio::null())
            .status()?;
        assert!(status.success());
        let plan = some_plan(&root);
        assert!(plan.also_confined.contains(&tmp));
        assert_eq!(
            plan.verify_confined(&plan.expected()),
            Err(IsolationError::DirectoryEscapesRoot)
        );
        fs::remove_dir(&tmp)?;
        Ok(())
    }

    #[test]
    fn an_unprepared_root_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let plan = some_plan(dir.path());
        // No marker.
        assert_eq!(
            plan.verify_root_prepared(),
            Err(IsolationError::UnpreparedRoot)
        );
        // A marker with other text.
        fs::write(dir.path().join(MARKER_FILE), "something else\n")?;
        assert_eq!(
            plan.verify_root_prepared(),
            Err(IsolationError::UnpreparedRoot)
        );
        // The exact marker.
        fs::write(dir.path().join(MARKER_FILE), MARKER_TEXT)?;
        assert_eq!(plan.verify_root_prepared(), Ok(()));
        // A missing root.
        let missing = some_plan(&dir.path().join("missing"));
        assert_eq!(
            missing.verify_root_prepared(),
            Err(IsolationError::UnpreparedRoot)
        );
        Ok(())
    }

    #[test]
    fn the_planned_directories_are_created_inside_the_root_and_the_report_is_written() -> TestResult
    {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join(MARKER_FILE), MARKER_TEXT)?;
        let plan = some_plan(dir.path());
        let expected = plan.expected();
        assert_eq!(plan.verify_confined(&expected), Ok(()));
        for directory in [
            &expected.config,
            &expected.data,
            &expected.local_data,
            &expected.cache,
            &expected.log,
        ] {
            assert!(directory.is_dir(), "{directory:?}");
        }
        assert_eq!(plan.write_report(&expected), Ok(()));
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join(REPORT_FILE))?)?;
        assert_eq!(report["verified"], true);
        assert_eq!(report["appConfigDir"], serde_json::json!(expected.config));
        assert_eq!(
            report["appLocalDataDir"],
            serde_json::json!(expected.local_data)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_link_that_leaves_the_root_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let root = dir.path().join("run");
        fs::create_dir(&root)?;
        let plan = some_plan(&root);
        std::os::unix::fs::symlink(outside.path(), root.join(CONFIG_DIR))?;
        assert_eq!(
            plan.verify_confined(&plan.expected()),
            Err(IsolationError::DirectoryEscapesRoot)
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_that_leaves_the_root_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let root = dir.path().join("run");
        fs::create_dir(&root)?;
        let plan = some_plan(&root);
        let junction = root.join(CONFIG_DIR);
        // `mklink /J` needs no privilege; it is the Windows link a profile folder could be.
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .stdout(std::process::Stdio::null())
            .status()?;
        assert!(status.success());
        assert_eq!(
            plan.verify_confined(&plan.expected()),
            Err(IsolationError::DirectoryEscapesRoot)
        );
        // Removing the junction must not touch its target.
        fs::remove_dir(&junction)?;
        assert!(outside.path().is_dir());
        Ok(())
    }

    /// The proof against Tauri itself: with the override applied to the context, the real path
    /// resolver (the Known Folder APIs on Windows) answers the five app directories inside the
    /// run root, while its base folders stay the real ones, and `verify` accepts the build.
    #[test]
    fn the_real_path_resolver_follows_the_override_and_verify_accepts_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join(MARKER_FILE), MARKER_TEXT)?;
        let root = fs::canonicalize(dir.path())?;
        let plan = some_plan(&strip_verbatim(&root));
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier = "io.example.jobniac-isolation-test".to_owned();
        plan.apply(context.config_mut());
        let app = tauri::test::mock_builder().build(context)?;
        let handle = app.handle();
        let path = handle.path();
        let expected = plan.expected();
        assert_eq!(path.app_config_dir()?, expected.config);
        assert_eq!(path.app_data_dir()?, expected.data);
        assert_eq!(path.app_local_data_dir()?, expected.local_data);
        assert_eq!(path.app_cache_dir()?, expected.cache);
        assert_eq!(path.app_log_dir()?, expected.log);
        // The base folders are not redirected: the app directories were, and only they.
        if let Ok(real_config) = path.config_dir() {
            assert!(!real_config.starts_with(&plan.root));
        }
        assert_eq!(plan.verify(handle), Ok(()));
        assert!(dir.path().join(REPORT_FILE).is_file());
        Ok(())
    }

    /// Without the override the same resolver answers real folders: the proof above is not vacuous.
    #[test]
    fn without_the_override_the_resolver_answers_real_folders() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = fs::canonicalize(dir.path())?;
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier = "io.example.jobniac-isolation-test".to_owned();
        let app = tauri::test::mock_builder().build(context)?;
        let path = app.handle().path();
        for resolved in [
            path.app_config_dir()?,
            path.app_local_data_dir()?,
            path.app_log_dir()?,
        ] {
            assert!(!resolved.starts_with(&root), "{resolved:?}");
        }
        Ok(())
    }

    fn strip_verbatim(path: &Path) -> PathBuf {
        let text = path.to_string_lossy();
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned())
    }
}
