# jobniac-testkit

Test support for the Jobniac workspace (dev-only crate, never published).

- `sqlite::TempDatabase`: a database file location inside a fresh temporary directory, removed on
  drop. It only names the location (`path()`, `as_str()`, `environment_pair()` for
  `JOBNIAC_DB_PATH`); the code under test creates, migrates and opens the file itself, exactly as the
  application does (`jobniac_runtime::database::open_database`, or `jobniac-cli migrate`).

All database tests of the workspace run in a plain `cargo test`: no server, no feature flag and no
environment variable is needed. The repositories' own tests (`crates/infra/tests/sqlite_*.rs`) use
a fixture of their own in `crates/infra/tests/sqlite_support`; the console tests (`crates/cli`) and
the composition tests use this crate.
