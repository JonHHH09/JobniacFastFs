# jobniac-testkit

Test support for the Jobniac workspace (dev-only crate, never published).

- `pg`: a disposable PostgreSQL 18 harness. `TestServer::from_env()` reads the cluster that
  `pg-test` exports; `fresh_database()` creates an empty database per test and drops it again.
- `runner` and the `pg-test` binary: `runner::run_cli` owns one disposable cluster for one run and
  runs `cargo test` against it. The binary is a thin wrapper; the logic lives in the library so its
  unit tests run under a plain `cargo test`.

## Running the PostgreSQL integration tests

Install PostgreSQL 18 locally, then point `JOBNIAC_POSTGRES_BIN` at its `bin` directory (the one that
contains `initdb`, `pg_ctl`, `pg_isready` and `postgres`). From the workspace root:

```text
JOBNIAC_POSTGRES_BIN=<PostgreSQL 18 bin dir> cargo run -p jobniac-testkit --bin pg-test -- -p jobniac-infra
```

Arguments after `--` go to `cargo test`. To run the ignored PostgreSQL tests of a crate, pass
`--ignored` to the test binaries as well:

```text
JOBNIAC_POSTGRES_BIN=<PostgreSQL 18 bin dir> cargo run -p jobniac-testkit --bin pg-test -- -p jobniac-testkit -- --ignored
```

What `pg-test` does:

1. Refuses the run (exit code 2) when `JOBNIAC_POSTGRES_BIN` is unset, a program is missing, or
   `postgres --version` does not report major version 18.
2. Creates the cluster directory under `JOBNIAC_PG_SCRATCH` when that is set and non-empty, otherwise
   under the platform temporary directory. It runs `initdb` with a random superuser password (SCRAM,
   TCP only), starts the server on a free loopback port and waits until `pg_isready` succeeds.
3. Runs `cargo test` with `JOBNIAC_TEST_PG_PORT`, `JOBNIAC_TEST_PG_USER` and
   `JOBNIAC_TEST_PG_PASSWORD` set, and with `JOBNIAC_TEST_DB=1` and the `JOBNIAC_DB_*` variables pointing
   only at the cluster's `postgres` database. The caller's `JOBNIAC_DB_*` values are removed. The
   password is never printed. The run is bounded by 30 minutes, which includes compiling the tests.
4. Stops the server (`pg_ctl stop -m fast`) and removes the directory. If the stop fails, the
   directory is kept for recovery and its path is printed to stderr (exit code 1).

On Ctrl-C or console close the test process tree is ended, and step 4 still runs.

The process exits with the `cargo test` exit code. When none of `JOBNIAC_TEST_PG_PORT`,
`JOBNIAC_TEST_PG_USER` and `JOBNIAC_TEST_PG_PASSWORD` is set, `TestServer::from_env()` returns `None`
and the test skips. When one of them is set but the configuration is invalid, the test panics, so it
fails instead of passing without a database.

## Running a command against the cluster (`pg-run`)

`pg-run` runs one command, not `cargo test`, against the same kind of disposable cluster. The end-to-end
suite uses it (`e2e/README.md`):

```text
JOBNIAC_POSTGRES_BIN=<PostgreSQL 18 bin dir> target/debug/pg-run node e2e/run.mjs
```

The first argument is the program and the rest are its arguments. The command gets `JOBNIAC_DB_*` and
`PG*` pointing at the cluster's `postgres` database as user `jobniac_test_admin`, and the bin directory
is put first on its `PATH` (so `psql` works). The host, port and user are printed to stderr; the password
is passed in the child environment only. The exit code is the command's, and the cluster is stopped and
removed afterwards, as for `pg-test`.

The caller's `JOBNIAC_DB_*` and `PG*` values are removed for both binaries. Before this change the
removal ran after the variables were set, so `pg-test` did not actually point `JOBNIAC_DB_*` at the
cluster; the order is fixed now.
