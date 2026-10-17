//! Process diagnostics of the desktop binary: the tracing subscriber, the opt-in log file and the
//! panic line.
//!
//! Every event the workspace emits is a fixed text with no path, value, key or token, so a sink
//! needs no redaction layer: it only has to stay quiet about the MCP SDK, whose debug and trace
//! events carry tool arguments and session ids.
//!
//! * The subscriber writes to stderr at `warn` (`RUST_LOG` overrides the level), which is what a
//!   terminal launch and `pnpm tauri dev` show. A Windows release build has no console, so stderr
//!   goes nowhere there.
//! * The file sink is opt-in: only the exact value `1` of `JOBNIAC_LOG_FILE` writes
//!   `jobniac.log` into the directory the caller passes (the per-user application log directory).
//!   The file is owner-only on Unix, appended to, and cut back to empty at startup once it
//!   exceeds [`LOG_FILE_MAX_BYTES`]; there is no rotation.
//! * The panic hook writes one fixed line (the source file name and line, never the payload) to
//!   stderr and to the log file when it is on. Release builds abort on panic, so the line is the
//!   only trace of the crash.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jobniac_core::env::Environment;
use jobniac_infra::workers::WorkerSupervisor;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// The opt-in switch of the log file: on only for the exact value `1`.
pub const LOG_FILE_VARIABLE: &str = "JOBNIAC_LOG_FILE";

/// The name of the log file inside the log directory.
pub const LOG_FILE_NAME: &str = "jobniac.log";

/// The size above which the log file is emptied at the next startup.
pub const LOG_FILE_MAX_BYTES: u64 = 256 * 1024;

/// Targets silenced whatever `RUST_LOG` says. The MCP SDK logs tool arguments, results, session
/// ids and raw request lines at debug and trace, so `rmcp` writes nothing; `hyper`
/// keeps only its warnings.
const SILENCED_TARGETS: [&str; 2] = ["rmcp=off", "hyper=warn"];

/// The level filter: `warn` by default, `RUST_LOG` on top, then the silenced targets.
fn env_filter() -> EnvFilter {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy();
    for target in SILENCED_TARGETS {
        if let Ok(directive) = target.parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// The log file path when the file sink is on and a directory is known.
#[must_use]
pub fn log_file_path(environment: &Environment, log_dir: Option<&Path>) -> Option<PathBuf> {
    log_dir
        .filter(|_| environment.flag_enabled(LOG_FILE_VARIABLE))
        .map(|dir| dir.join(LOG_FILE_NAME))
}

/// Opens the log file for appending, creating its directory and the file (owner-only on Unix). A
/// file above [`LOG_FILE_MAX_BYTES`] is emptied first.
///
/// # Errors
///
/// Returns the IO error when the directory or the file cannot be prepared.
pub fn open_log_file(path: &Path) -> io::Result<File> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    if fs::metadata(path).is_ok_and(|meta| meta.len() > LOG_FILE_MAX_BYTES) {
        // Emptied once, then reopened for appending, so the handle never keeps a stale offset.
        let mut options = OpenOptions::new();
        options.write(true).truncate(true);
        restrict_to_owner(&mut options);
        options.open(path)?;
    }
    open_log_file_for_append(path)
}

/// Opens the log file for appending without ever emptying it (the panic hook, which runs after
/// startup and must not wipe the history it adds to).
///
/// # Errors
///
/// Returns the IO error when the file cannot be opened.
pub fn open_log_file_for_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    restrict_to_owner(&mut options);
    options.open(path)
}

fn restrict_to_owner(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = options;
}

/// Installs the process subscriber: stderr always, and the log file too when `log_file` holds a
/// path that opens. A subscriber can be installed once; a second call leaves the first in place.
pub fn init_tracing(log_file: Option<&Path>) {
    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter())
        .with_ansi(false);
    let file = log_file.and_then(|path| open_log_file(path).ok());
    let _ = match file {
        Some(file) => builder
            .with_writer(io::stderr.and(Mutex::new(file)))
            .try_init(),
        None => builder.with_writer(io::stderr).try_init(),
    };
}

/// The worker supervisor of this process. A process whose own executable cannot be located gets
/// `None`, and the PDF, link and Arbeitnow operations then answer unavailable; the cause is logged
/// once per composition as a fixed text, so the degraded features can be explained.
#[must_use]
pub fn current_process_supervisor() -> Option<Arc<WorkerSupervisor>> {
    match WorkerSupervisor::for_current_process() {
        Ok(supervisor) => Some(Arc::new(supervisor)),
        Err(_) => {
            tracing::warn!("worker supervisor unavailable");
            None
        }
    }
}

/// The fixed line a panic leaves: the source file name and line, never the payload or a directory.
#[must_use]
pub fn panic_line(info: &PanicHookInfo<'_>) -> String {
    match info.location() {
        Some(location) => {
            let file = Path::new(location.file())
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown");
            format!("jobniac: internal error at {file}:{}", location.line())
        }
        None => "jobniac: internal error".to_owned(),
    }
}

/// Replaces the panic hook with one that writes [`panic_line`] to stderr and, when `log_file` is
/// set, appends it there. The hook runs before a release build aborts. A debug build also runs
/// the previous hook, so a developer keeps the message, payload and backtrace. It sets the
/// process-global hook: callers (and tests) must not race on it.
pub fn install_panic_hook(log_file: Option<PathBuf>) {
    #[cfg(debug_assertions)]
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = panic_line(info);
        let _ = writeln!(io::stderr(), "{line}");
        if let Some(mut file) = log_file
            .as_deref()
            .and_then(|path| open_log_file_for_append(path).ok())
        {
            let _ = writeln!(file, "{line}");
        }
        #[cfg(debug_assertions)]
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn environment(pairs: &[(&str, &str)]) -> Environment {
        Environment::from_os_pairs(
            pairs
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into())),
            false,
        )
    }

    #[test]
    fn the_file_sink_is_on_only_for_the_exact_value_one() {
        let dir = Path::new("/log-dir");
        for (value, expected) in [("1", true), ("true", false), ("0", false), ("", false)] {
            let env = environment(&[(LOG_FILE_VARIABLE, value)]);
            assert_eq!(
                log_file_path(&env, Some(dir)).is_some(),
                expected,
                "{value}"
            );
        }
        assert!(log_file_path(&environment(&[]), Some(dir)).is_none());
        let on = environment(&[(LOG_FILE_VARIABLE, "1")]);
        assert!(log_file_path(&on, None).is_none());
        assert_eq!(log_file_path(&on, Some(dir)), Some(dir.join(LOG_FILE_NAME)));
    }

    #[test]
    fn the_log_file_is_created_appended_and_emptied_when_oversized() -> TestResult {
        let dir = std::env::temp_dir().join(format!("jobniac-diag-{}", uuid::Uuid::new_v4()));
        let path = dir.join("logs").join(LOG_FILE_NAME);
        writeln!(open_log_file(&path)?, "first")?;
        writeln!(open_log_file(&path)?, "second")?;
        assert_eq!(fs::read_to_string(&path)?, "first\nsecond\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        }
        let filler = vec![b'x'; usize::try_from(LOG_FILE_MAX_BYTES)? + 1];
        fs::write(&path, filler)?;
        writeln!(open_log_file(&path)?, "after")?;
        assert_eq!(fs::read_to_string(&path)?, "after\n");
        // The panic hook's append open keeps an oversized file, and a later write lands after it.
        let filler = vec![b'y'; usize::try_from(LOG_FILE_MAX_BYTES)? + 1];
        fs::write(&path, &filler)?;
        writeln!(open_log_file_for_append(&path)?, "panic")?;
        let kept = fs::read(&path)?;
        assert_eq!(kept.len(), filler.len() + "panic\n".len());
        assert!(kept.ends_with(b"panic\n"));
        // After the startup truncation the handle appends: a second writer does not overwrite.
        let mut first = open_log_file(&path)?;
        writeln!(first, "one")?;
        writeln!(open_log_file_for_append(&path)?, "two")?;
        writeln!(first, "three")?;
        assert_eq!(fs::read_to_string(&path)?, "one\ntwo\nthree\n");
        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn a_panic_leaves_only_the_file_name_and_line_never_the_payload() {
        use std::sync::Arc;
        let seen = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&seen);
        // The panic hook is process-global and other tests panic on purpose on their own threads
        // while this one runs. The probe hook records only this thread's panic and hands every
        // other panic to the hook it replaced.
        let test_thread = std::thread::current().id();
        let previous = Arc::new(std::panic::take_hook());
        let chained = Arc::clone(&previous);
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().id() == test_thread {
                if let Ok(mut line) = sink.lock() {
                    *line = panic_line(info);
                }
            } else {
                (*chained)(info);
            }
        }));
        let result = std::panic::catch_unwind(|| {
            std::panic::panic_any("secret-token-in-the-payload".to_owned());
        });
        std::panic::set_hook(Box::new(move |info| (*previous)(info)));
        assert!(result.is_err());
        let line = seen.lock().map(|line| line.clone()).unwrap_or_default();
        assert!(line.starts_with("jobniac: internal error at diagnostics.rs:"));
        assert!(!line.contains("secret-token") && !line.contains('/') && !line.contains('\\'));
    }
}
