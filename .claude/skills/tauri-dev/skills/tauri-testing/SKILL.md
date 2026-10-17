---
name: tauri-testing
description: Testing Tauri 2 apps - frontend unit tests with the mock runtime (`mockIPC`, `mockWindows`, `clearMocks`, `shouldMockEvents` from `@tauri-apps/api/mocks`, Vitest/Jest) and end-to-end WebDriver tests (`@wdio/tauri-service`, WebdriverIO, Selenium, `tauri-driver`, msedgedriver, WebKitWebDriver, xvfb on CI). Load when asked to add tests, mock `invoke` or Tauri events/windows, set up e2e or WebDriver, run tests in GitHub Actions, or ask which platforms WebDriver supports.
---

# Tauri Testing

These instructions and the files in `references/` are local copies of the official Tauri v2 docs (https://v2.tauri.app/develop/tests/, tauri-docs 4b74da6, 2026-10-13). Work only from them: before writing code or giving advice, read the reference file(s) for the task; cite the page URL from the file's Source header. If the references don't cover something, say so and point to the live page instead of filling the gap from memory.

## Project context
- There is no test suite yet in this project; the frontend is React 19 + TypeScript + Vite in `src/`, pnpm as package manager.
- The docs examples use Vitest but state any frontend testing library such as jest works (tests-mocking.md).
- The app is built with `pnpm tauri build`; Rust crate is in `src-tauri/`.
- Dev machine is Windows: the `tauri-driver` route needs a matching Microsoft Edge Driver (tests-webdriver-manual-setup.md).

## Key rules
- Two test levels: unit/integration with a mock runtime (native webview libraries are not executed) and end-to-end via the WebDriver protocol (tests.md).
- Mocks live in `@tauri-apps/api/mocks`: `mockIPC`, `mockWindows`, `clearMocks` (tests-mocking.md).
- Always clear mocks after each test (`clearMocks()`) to undo mock state between runs (tests-mocking.md).
- `mockIPC((cmd, args) => ...)` intercepts `invoke`; return a value to simulate the Rust command result (tests-mocking.md).
- jsdom has no WebCrypto: define `window.crypto.getRandomValues` (e.g. with `randomFillSync` from `crypto`) in `beforeAll` before using `mockIPC`/`mockWindows` (tests-mocking.md).
- To spy on calls, combine `mockIPC` with `vi.spyOn(window.__TAURI_INTERNALS__, "invoke")` (tests-mocking.md).
- Sidecar/shell mocks: read the callback id from `args.message.onEventFn`, emit `Stdout` events, and end with a `Terminated` event or the promise never resolves (tests-mocking.md).
- Event mocking (since v2.7.0) is partial: `mockIPC(() => {}, { shouldMockEvents: true })`; `emitTo` and `emit_filter` are not supported (tests-mocking.md).
- `mockWindows('main', 'second', ...)`: first label is the "current" window; it only fakes window existence, not properties (use `mockIPC` for those) (tests-mocking.md).
- For command mocking in e2e tests, `@wdio/tauri-service` provides `browser.tauri.mock()` (tests-mocking.md).
- Recommended e2e path: WebdriverIO with `@wdio/tauri-service`, works on Windows, Linux and macOS (tests-webdriver.md).
- The service defaults to an embedded WebDriver server in the app (`driverProvider: 'embedded'`, needs plugin `tauri-plugin-wdio-webdriver`); alternatives are `tauri-driver` (Windows/Linux) or CrabNebula's fork (paid API key for macOS) (tests-webdriver.md).
- `tauri-plugin-wdio` enables `browser.tauri.execute()`, IPC mocking and log capture (tests-webdriver.md).
- Scaffold with `npm create wdio@latest ./`, choose Desktop Testing and Tauri (tests-webdriver.md).
- Browser mode runs the frontend in plain Chrome against a Vite dev server, with no Tauri binary or driver, and intercepts `invoke()` (tests-webdriver.md).
- Driving `tauri-driver` directly supports only Windows and Linux on desktop (no macOS WKWebView driver); iOS/Android via Appium 2 is not streamlined (tests-webdriver-manual-setup.md).
- Install with `cargo install tauri-driver --locked`; Linux needs `WebKitWebDriver` (e.g. `webkit2gtk-driver` on Debian-based) (tests-webdriver-manual-setup.md).
- Windows needs an `msedgedriver.exe` matching your Edge version on `$PATH` (or `--native-driver`); a mismatch can make the suite hang while connecting (tests-webdriver-manual-setup.md).
- Direct-driver capabilities: `tauri:options` with `application` path to the binary; Selenium also sets browser name `wry` and server `http://127.0.0.1:4444/` (tests-webdriver-example-selenium.md, tests-webdriver-example-webdriverio.md).
- Examples build with `tauri build --debug --no-bundle` first and expect `src-tauri/target/debug/<app>` (tests-webdriver-example-selenium.md, tests-webdriver-example-webdriverio.md).
- CI: Linux runs under a fake display (`xvfb-run`) with `webkit2gtk-driver` and `xvfb` installed; Windows runs directly; run `cargo test` before WebDriver tests (tests-webdriver-ci.md).
- `tauri-driver` version is independent of other Tauri versions (tests-webdriver-ci.md).
- Docs inconsistency in the WebdriverIO example: `specs` are `./develop/tests/specs/**/*.js` in the config but described as `./test/specs/**/*.js`, and its `package.json` snippet has a missing closing quote on `@wdio/local-runner`; fix these when copying (tests-webdriver-example-webdriverio.md).
- The `tauri-action` GitHub Action helps run CI, but any CI runner works if each platform has the libraries needed to compile (tests.md).

## How to mock IPC in frontend unit tests
1. Install a test runner (docs use Vitest) and a DOM environment; add the `window.crypto` shim in `beforeAll`.
2. In the test call `mockIPC((cmd, args) => { if (cmd === "add") return args.a + args.b; })` before calling `invoke("add", {...})` (tests-mocking.md).
3. Add `afterEach(() => clearMocks())`.
4. For events pass `{ shouldMockEvents: true }`; for multiple windows call `mockWindows(...)`.

## How to set up WebDriver e2e
1. Preferred: follow https://webdriver.io/docs/desktop-testing/tauri (Plugin Setup) with `@wdio/tauri-service` and `appBinaryPath` (tests-webdriver.md).
2. Manual: install `tauri-driver`, platform driver (WebKitWebDriver / msedgedriver), create `e2e-tests/` with Selenium (Mocha + Chai + `selenium-webdriver`) or WebdriverIO (Mocha, spec reporter) (tests-webdriver-manual-setup.md and the examples).
3. In the test config, build the debug binary, start `tauri-driver` before the session, and kill it afterwards.
4. On CI copy the GitHub Actions workflow in tests-webdriver-ci.md.

## Reference index
| File | Read it when |
|---|---|
| [tests.md](references/tests.md) | choosing between mock runtime and WebDriver |
| [tests-mocking.md](references/tests-mocking.md) | `mockIPC`, `mockWindows`, events, sidecar mocks |
| [tests-webdriver.md](references/tests-webdriver.md) | WebDriver overview, `@wdio/tauri-service`, platform support |
| [tests-webdriver-manual-setup.md](references/tests-webdriver-manual-setup.md) | installing and driving `tauri-driver` yourself |
| [tests-webdriver-ci.md](references/tests-webdriver-ci.md) | GitHub Actions workflow for WebDriver tests |
| [tests-webdriver-example-selenium.md](references/tests-webdriver-example-selenium.md) | Selenium + Mocha example |
| [tests-webdriver-example-webdriverio.md](references/tests-webdriver-example-webdriverio.md) | manual WebdriverIO example config and spec |

## Related skills
tauri-ipc (commands, events being mocked), tauri-develop (dev workflow, debug builds), tauri-distribute (CI pipelines, tauri-action), tauri-architecture, tauri-security, tauri-plugin-development, tauri-official-plugins.
