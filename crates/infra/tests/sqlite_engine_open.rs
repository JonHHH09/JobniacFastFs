//! Opening the SQLite file: concurrent first starts, foreign files, and file permissions. All data
//! is synthetic.

use std::path::Path;
use std::sync::{Arc, Barrier};

use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::migrations::{APPLICATION_ID, migrate};
use jobniac_infra::sqlite::pool::SqlitePool;
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn journal_mode(path: &Path) -> Result<String, rusqlite::Error> {
    Connection::open(path)?.pragma_query_value(None, "journal_mode", |row| row.get(0))
}

fn sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[test]
fn processes_starting_together_on_one_fresh_file_all_succeed() -> TestResult {
    const STARTERS: usize = 8;
    for round in 0..6 {
        let directory = tempfile::tempdir()?;
        let path = Arc::new(directory.path().join(format!("fresh-{round}.sqlite3")));
        let barrier = Arc::new(Barrier::new(STARTERS));
        let starters: Vec<_> = (0..STARTERS)
            .map(|_| {
                let path = Arc::clone(&path);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || -> Result<(), String> {
                    barrier.wait();
                    migrate(&path).map_err(|error| format!("migrate: {error}"))?;
                    SqlitePool::open(path.as_path())
                        .map(|_| ())
                        .map_err(|error| format!("open: {error}"))
                })
            })
            .collect();
        for starter in starters {
            starter
                .join()
                .map_err(|_| "a starter panicked")?
                .map_err(|error| format!("round {round}: {error}"))?;
        }
        assert_eq!(journal_mode(&path)?, "wal");
    }
    Ok(())
}

fn assert_refused_untouched(path: &Path) -> TestResult {
    assert!(migrate(path).is_err(), "migrate refuses the file");
    assert!(matches!(
        SqlitePool::open(path),
        Err(SqliteError::Unavailable)
    ));
    assert_eq!(journal_mode(path)?, "delete", "the journal mode is kept");
    assert!(!sidecar(path, "-wal").exists(), "no -wal file is left");
    assert!(!sidecar(path, "-shm").exists(), "no -shm file is left");
    let tables: i64 = Connection::open(path)?.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    assert!(tables <= 1, "nothing was added");
    Ok(())
}

#[test]
fn a_foreign_file_with_tables_is_refused_and_not_converted_to_wal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("other-app.sqlite3");
    Connection::open(&path)?.execute_batch("CREATE TABLE notes (body TEXT)")?;
    assert_refused_untouched(&path)
}

#[test]
fn a_foreign_application_id_is_refused_and_not_converted_to_wal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("other-id.sqlite3");
    Connection::open(&path)?.pragma_update(None, "application_id", APPLICATION_ID + 1)?;
    assert_refused_untouched(&path)
}

#[test]
fn a_foreign_user_version_is_refused_and_not_converted_to_wal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("other-version.sqlite3");
    Connection::open(&path)?.pragma_update(None, "user_version", 7)?;
    assert_refused_untouched(&path)
}

#[test]
fn an_empty_existing_file_is_accepted_and_becomes_wal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("empty.sqlite3");
    std::fs::write(&path, b"")?;
    migrate(&path)?;
    SqlitePool::open(&path)?;
    assert_eq!(journal_mode(&path)?, "wal");
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_database_and_its_wal_files_are_owner_only() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| -> std::io::Result<u32> {
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("private.sqlite3");
    migrate(&path)?;
    let pool = SqlitePool::open(&path)?;
    assert_eq!(mode(&path)?, 0o600);
    assert_eq!(mode(&sidecar(&path, "-wal"))?, 0o600);
    assert_eq!(mode(&sidecar(&path, "-shm"))?, 0o600);
    drop(pool);

    // A looser existing file is tightened when it is opened.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
    let _pool = SqlitePool::open(&path)?;
    assert_eq!(mode(&path)?, 0o600);
    Ok(())
}
