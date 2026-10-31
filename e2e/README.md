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
  per-user Settings file (`settings.json`) and the Arbeitnow keys file (`feature-keys.json`) are
  redirected into the run root on Linux (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`;
  `HOME` and `XDG_RUNTIME_DIR` are kept). **On Windows they are not redirected**: Tauri resolves them
  through Known Folder APIs that ignore `APPDATA` and `LOCALAPPDATA`, so the default session there
  assumes the switches in the developer's real `settings.json` are at their defaults (on).
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
