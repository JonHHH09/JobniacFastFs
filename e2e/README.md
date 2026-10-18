# End-to-end suite

WebDriver tests of the desktop workflows against the real debug build of `jobniac`, through
`tauri-driver` (see the [Tauri WebDriver guide](https://v2.tauri.app/develop/tests/webdriver/)).
The suite uses plain WebdriverIO and mocha; the `@wdio/tauri-service` is not used, so the
application needs no test plugin.

## What it covers

Two application sessions run in order on one disposable database.

| Session | Spec | Steps |
|---|---|---|
| default (every opt-in flag unset) | `specs/app.spec.mjs` | 1 health UP; 2 create a profile with personal details and one experience, search it, edit and save (`Profile saved.`), unsaved-edit discard prompt; 3 paste a job posting and see it in the list; 4 analyze the pair and open the match report; 7 opt-in controls absent or disabled; 8 a busy search control stays disabled while the database lock holds, no file path or token in the page |
| generation (`JOBNIAC_GENERATED_ENABLED=1`, generated root in the run folder) | `specs/generation.spec.mjs` | 6 generate a standalone PDF and see the output rows |

Step 5 (import a PDF through the Documents screen) is `it.skip`: see Deviations.

## Running it

Prerequisites: Node 24, pnpm 12.10.1, Rust (stable), PostgreSQL 18 binaries, `tauri-driver`
(`cargo install tauri-driver --locked`), and a WebDriver for the platform: `msedgedriver` matching the
installed WebView2 runtime on Windows, `WebKitWebDriver` (package `webkit2gtk-driver`) and `xvfb` on Linux.

```text
pnpm install --frozen-lockfile
pnpm tauri build --debug --no-bundle
cargo build --locked -p jobniac-cli -p jobniac-testkit --bins
JOBNIAC_POSTGRES_BIN=<PostgreSQL 18 bin directory> target/debug/pg-run node e2e/run.mjs
```

`pnpm e2e` is the same runner (`node e2e/run.mjs`) and needs the disposable database environment that
`pg-run` provides. `pg-run` (crates/testkit, see its README) starts a fresh PostgreSQL 18 cluster on a
free loopback port, runs the command with `JOBNIAC_DB_*` and `PG*` pointing only at that cluster,
prints the host, port and user (never the password), and stops and removes the cluster afterwards.
Run it on Linux under `xvfb-run` when there is no display. On Windows put the msedgedriver folder on
`PATH` first.

`run.mjs` migrates the database, registers the generated root with `jobniac-cli setup-generated-root`,
then runs each session. The application binary is `target/debug/jobniac(.exe)` unless
`JOBNIAC_E2E_APP` is set; the console binary is `target/debug/jobniac-cli(.exe)` or `JOBNIAC_E2E_CLI`.

Failure screenshots go to `e2e/.artifacts/` (ignored by git); the tauri-driver log tail of a failed
test is printed to the console. The run root (`target/e2e-*`) is removed at the end of every run.

## Deviations and gaps

- **Step 5 (PDF import) is not automated.** `document_choose_file` opens the native open dialog through
  `NativePdfPicker`; WebDriver cannot drive that dialog, and the `PdfPicker` trait has no test seam in
  the desktop shell (the fake exists only in the Rust unit tests). The import root
  (`JOBNIAC_DOCUMENT_IMPORT_ROOT`) only sets the dialog's start folder, so it does not help. Closing
  this needs a decision on a test-only picker seam (for example an env-gated picker).
- **The window close gate is not exercised.** WebDriver's window close destroys the window without
  the `CloseRequested` event, so the Rust gate never refuses. The discard prompt is covered in the
  page (step 2, `UnsavedChangesDialog`), and the gate itself is covered by the Rust tests.
- **Opt-in controls are disabled, not absent.** With the flags unset the artifact and generation
  controls are rendered disabled. The default desktop is meant to show no optional UI. The
  suite accepts absent or disabled and records the difference.
- **The generated root is not in the user temporary folder.** On the Windows host,
  `setup-generated-root` refused a root under `%TEMP%`, under `C:\` and under `%LOCALAPPDATA%` with
  "could not be initialized", and accepted the same empty directory under the repository's `target/`
  folder. The suite therefore uses `target/e2e-*`. The cause is not diagnosed; it is a finding for the
  owner, because the default per-user generated root sits under `%LOCALAPPDATA%`.
- **The debug build has no sidecar and no notices.** The sidecar (`binaries/jobniac-cli`) and the
  notices resource are added only by `src-tauri/tauri.release.conf.json`, which the release build
  passes with `--config`; the debug build does not use it. The suite runs the console binary from
  `target/debug` and PDFium from `vendor/pdfium` (`JOBNIAC_PDFIUM_DIR`).
- **The busy-state check uses a database lock.** A held ACCESS EXCLUSIVE lock on `profile.profiles`
  keeps the search waiting in the database; the control must be disabled meanwhile. The lock holder
  is terminated at the end of the check.

## Environment and privacy

Every name and address in the suite is synthetic. The canaries that must never reach the page are the
run's import and generated roots (both slash forms) and the disposable database password; the check
reads the page text and every attribute value. The suite refuses any database host other than
`127.0.0.1` and any run that is not started through `pg-run`.

## CI

`.github/workflows/e2e.yml` runs the same steps on `windows-latest` and `ubuntu-24.04`. The Windows job
installs `msedgedriver` from the Microsoft endpoint for the runner's WebView2 version and checks its
Authenticode signature. The Linux job installs `webkit2gtk-driver` and `xvfb`, and PostgreSQL 18 through
`scripts/ci/install-postgresql-18.sh`. Neither job has been run: the GitHub Actions runners are not
available for this account, so the workflow is validated with actionlint only. The Windows PostgreSQL
install through Chocolatey (`postgresql18`) is unverified.
