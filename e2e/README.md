# End-to-end suite

WebDriver tests of the desktop workflows against the real debug build of `jobniac`, through
`tauri-driver` (see the [Tauri WebDriver guide](https://v2.tauri.app/develop/tests/webdriver/)).
The suite uses plain WebdriverIO and mocha; the `@wdio/tauri-service` is not used, so the
application needs no test plugin.

## What it covers

Three application sessions run in order on one temporary SQLite database file.

| Session | Spec | Steps |
|---|---|---|
| default (no feature flag set: the desktop defaults, every optional feature on) | `specs/app.spec.mjs` | 1 health UP; 2 create a profile with personal details and one experience, search it, edit and save (`Profile saved.`), unsaved-edit discard prompt; 3 paste a job posting and see it in the list; 4 analyze the pair and open the match report; 7 the add-job-from-link, delete-profile and Arbeitnow scan controls are present; 8 a busy save control stays disabled while the database write lock is held, no file path or token in the page |
| opt-out (the five `JOBNIAC_*_ENABLED` feature flags set to `0`) | `specs/optout.spec.mjs` | 7 the Arbeitnow, link and delete controls are absent (in the editors of the saved profile and job); the artifact and generation controls are disabled. Each check first waits for the matching `*_capabilities_get` command to report the feature unavailable |
| generation (`JOBNIAC_GENERATED_ENABLED=1`, generated root in the run folder) | `specs/generation.spec.mjs` | 6 generate a standalone PDF and see the output rows |

Step 5 (import a PDF through the Documents screen) is `it.skip`: see Deviations.

## Running it

Prerequisites: Node 24, pnpm 12.10.1, Rust (stable), `tauri-driver`
(`cargo install tauri-driver --locked`), and a WebDriver for the platform: `msedgedriver` matching the
installed WebView2 runtime on Windows, `WebKitWebDriver` (package `webkitgtk-webdriver` on Ubuntu 26.04, `webkit2gtk-driver` on older
releases) and `xvfb` on Linux.

```text
pnpm install --frozen-lockfile
pnpm tauri build --debug --no-bundle --config src-tauri/tauri.e2e.conf.json
cargo build --locked -p jobniac-cli
node e2e/run.mjs
```

`pnpm e2e` is the same runner (`node e2e/run.mjs`). It needs no database server: `run.mjs` names a
fresh SQLite file in its run folder (`target/e2e-*/jobniac.sqlite3`) and passes it to the console
binary and to the application as `JOBNIAC_DB_PATH`. A `JOBNIAC_DB_PATH` of the caller is ignored. The
lookups of the specs (the identifiers of the profile and the job) and the lock of the busy-state
check use `node:sqlite` (built into Node 24), so no `sqlite3` program is needed.
Run it on Linux under `xvfb-run` when there is no display. On Windows put the msedgedriver folder on
`PATH` first.

`run.mjs` migrates the database file, registers the generated root with `jobniac-cli setup-generated-root`,
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
- **The optional features are on in the default session.** The desktop app turns them on unless the
  process sets a flag; the opt-out session sets them to `0`. Every session names the run's generated
  root (`JOBNIAC_GENERATED_ROOT`), so the app never registers the machine's per-user root. The
  per-user Settings file (`settings.json`), the Arbeitnow keys file (`feature-keys.json`) and the
  WebView2 profile are redirected into the run root on every platform by the run isolation of the
  debug build (see "Run isolation" below); on Linux the XDG variables are redirected as well
  (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`; `HOME` and `XDG_RUNTIME_DIR` are kept).
  The suite does not exercise the MCP tools or turn the three MCP opt-ins (`mcpArbeitnow`,
  `mcpLinkFetch`, `mcpDeletion`) on.
- **The generated root is not in the user temporary folder.** On the Windows host,
  `setup-generated-root` refused a root under `%TEMP%`, under `C:\` and under `%LOCALAPPDATA%` with
  "could not be initialized", and accepted the same empty directory under the repository's `target/`
  folder. The suite therefore uses `target/e2e-*`. The cause is not diagnosed; it is a finding for the
  owner, because the default per-user generated root sits under `%LOCALAPPDATA%`. On the CI runner
  the workspace is on `D:\`, whose inherited ACL grants Authenticated Users write access, so the
  runner gives the generated root a protected DACL (current user, SYSTEM, Administrators) with
  `icacls` before setup (`e2e/support/config.mjs`).
- **The debug build has no sidecar and no notices.** The sidecar (`binaries/jobniac-cli`) and the
  notices resource are added only by `src-tauri/tauri.release.conf.json`, which the release build
  passes with `--config`; the debug build does not use it. The suite runs the console binary from
  `target/debug` and PDFium from `vendor/pdfium` (`JOBNIAC_PDFIUM_DIR`).
- **The E2E build opens a WebView2 debugging port.** msedgedriver attaches to the application
  through the `DevToolsActivePort` file, which WebView2 writes only when it runs with
  `--remote-debugging-port`. Without it the session fails with "session not created:
  DevToolsActivePort file doesn't exist". `src-tauri/tauri.e2e.conf.json` adds
  `--remote-debugging-port=0` to the main window's `additionalBrowserArgs` (keeping Tauri's default
  `--disable-features` list, which the setting replaces), and only the E2E build passes it with
  `--config`; release builds never open the port. `--config` replaces the `windows` array as a whole,
  so that file repeats the main window entry of `tauri.conf.json` and must follow its changes.
- **The busy-state check uses the database write lock.** The spec holds `BEGIN IMMEDIATE` on the
  run's database file (from the test process) and saves a job posting; the write waits for the lock,
  so the save control must be disabled meanwhile. The lock is released (rolled back) after that.
  Reads are not blocked in WAL mode, so a search cannot be used for this check as it could with a
  table lock.

## Run isolation

On Windows, Tauri resolves the app config, data, local data, cache and log directories through the
Known Folder APIs, which ignore `APPDATA` and `LOCALAPPDATA`, and the WebView2 profile (the browser
session) defaults to the app local data directory. A test run therefore redirects them inside the
process: the debug build reads `JOBNIAC_TEST_RUN_ROOT` and sets Tauri's `app.appDirectoriesOverride`
to five folders under that root (`app-config`, `app-data`, `app-local-data`, `app-cache`,
`app-log`) before the application is built (`src-tauri/src/test_isolation.rs`).

- **Fail closed.** The variable only counts when it is present. A value that is not a clean absolute
  path, a root without the marker file `.jobniac-test-run` (written by `createRunRoot`), a missing
  `JOBNIAC_DB_PATH`, `JOBNIAC_GENERATED_ROOT` or temporary directory (`TMP` and `TEMP` on Windows,
  `TMPDIR` elsewhere), any of those (or `JOBNIAC_DOCUMENT_IMPORT_ROOT`, `WEBVIEW2_USER_DATA_FOLDER`)
  outside the root, a resolved directory that is not the planned one, a root that overlaps a real
  per-user folder, or a link or junction that leaves the root ends the start with exit status 1 and
  a fixed message, before any window exists. Nothing falls back to the real folders.
- **WebView2 profile.** On Windows msedgedriver starts the application with
  `WEBVIEW2_USER_DATA_FOLDER` set to the profile of the session, and without `userDataFolder` in
  `webviewOptions` it makes a temporary one outside the run root, which the check above refuses
  ("Microsoft Edge failed to start: was killed"). Each WebDriver session therefore names a new
  `webview2-*` folder under the root (`webviewUserDataFolder` in `support/isolation.mjs`).
- **Debug builds only.** The module and every use of it sit under `cfg(debug_assertions)`; a release
  build has no code and no variable name for it and ignores `JOBNIAC_TEST_RUN_ROOT`
  (`config_tests::the_run_isolation_is_compiled_out_of_release_builds`). So the runner accepts only a
  binary in a `debug` folder and checks the application's own report afterwards.
- **Proof before launch.** `proveBeforeLaunch` (`support/isolation.mjs`) runs before anything is
  started and throws unless the root is a prepared folder under `target/` that overlaps no real
  per-user base or application folder and every path the application will use is inside it.
  `node e2e/native-smoke.mjs --prove-only` runs that proof alone. The Rust test
  `the_real_path_resolver_follows_the_override_and_verify_accepts_it` checks the real Tauri path
  resolver against the override without opening a window.
- **Proof after the run.** The application writes `isolation-report.json` into the run root after it
  verified the redirection; the runner requires it, compares it with the planned layout and shows
  that the real application folders (`%APPDATA%\<identifier>`, `%LOCALAPPDATA%\<identifier>`) are
  unchanged (names, sizes and modification times only; no file is opened).

`node --test e2e/support/isolation.test.mjs` tests the proof functions without any application.

## Native smoke

`node e2e/native-smoke.mjs` launches the debug `jobniac` binary once, isolated as above, with the
packaging smoke quit hook (`JOBNIAC_SMOKE_QUIT_AFTER_LOAD=1`) and `JOBNIAC_LOG_FILE=1`. It needs
`cargo build -p jobniac-desktop -p jobniac-cli` and no WebDriver.

What it covers: the Windows binary starts under the redirected directories, the native window and
WebView2 open and the page loads, the health use case answers, the process exits 0; the settings
load from an empty redirected config directory (defaults), the Arbeitnow keys file is generated
in the redirected local data directory, the log file is written to the redirected log directory,
the WebView2 profile lives in the redirected local data directory, the database is migrated and
used under the run root, and the real application folders are unchanged.

What it does not cover: any screen or workflow (use `pnpm e2e` for those), the MCP server and its
child (off by default, not started), the updater (not configured in a debug build), the native file
dialogs (an open or save dialog keeps its most-recently-used folder in the Windows registry, which
this does not redirect and does not exercise), the release build, the installer and the sidecar,
PDF import, rendering and generation through the real PDFium, and the `tauri-driver` path (`pnpm
e2e` passes the same isolation variables, but that runner was not run for this change). Shared WebView2 and
Edge state outside the user data folder (the WebView2 runtime itself, Windows Error Reporting) is
not redirected.

## Environment and privacy

Every name and address in the suite is synthetic. The canaries that must never reach the page are the
run's import and generated roots and its database file (all in both slash forms); the check reads the
page text and every attribute value. The suite refuses any database file outside the run folder
under `target/`.

## Platforms

The WebDriver suite runs on Windows and Linux only. `tauri-driver` has no macOS driver, because WKWebView
has no WebDriver server that Tauri can drive. On macOS the app is covered by the Rust job (`Rust
(macos-latest)`, including the sandboxed PDF worker tests) and, for a release, by the scripted smoke of the
built DMG (`scripts/release/macos_smoke.py`: launch with the smoke quit hook, CLI, MCP and a PDF
extraction through the bundled PDFium).

## CI

`.github/workflows/e2e.yml` runs the same steps on `windows-latest` and `ubuntu-26.04`. The Windows job
installs `msedgedriver` from the Microsoft endpoint for the runner's WebView2 version and checks its
Authenticode signature. The Linux job installs `webkitgtk-webdriver` and `xvfb`.
