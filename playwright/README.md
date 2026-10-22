# Browser UI suite (Playwright)

Checks the React UI in Chromium against the in-memory mock IPC (`src/dev`, started by
`pnpm dev:mock`). It covers rendering, navigation, forms, validation, dialogs and layout. It does
not cover the Rust core, the database or the native webview: those stay with the WebDriver suite in
`e2e/` and the Rust tests.

## Run

```text
pnpm exec playwright install chromium   # once, or set PLAYWRIGHT_CHROMIUM_PATH=/usr/bin/chromium
pnpm test:ui                            # headless, starts the mock server on port 1431 itself
pnpm test:ui:headed                     # watch the browser
pnpm test:ui:watch                      # Playwright UI mode: live runner, time travel, re-run on save
pnpm exec playwright test profiles      # one spec file
pnpm exec playwright show-report        # HTML report (CI only writes it)
```

A running `pnpm dev:mock --port 1431 --strictPort` is reused locally. Failures keep a screenshot and
a trace under `playwright/.artifacts/` (ignored by git); open a trace with
`pnpm exec playwright show-trace <trace.zip>`.

## How it is built

- `fixtures.ts`: `open(section, params)` loads `/?section=<id>&mockLatency=0`, and an automatic
  fixture fails any test that causes a page error or a console error.
- Mock switches (`src/dev/README.md`) set up states from the URL: `mockFail=<command>`,
  `mockEmpty=1`, `mockUpdate=1`. Command names are the snake_case Tauri commands the mock handles
  (`list_profiles`, `match_analyze_pair`, `arbeitnow_scan`, `health_check`).
- Selectors are roles and accessible names first, `data-testid` where the page has one.
- Every wait is a web-first assertion; nothing sleeps.
- Each test starts with a fresh page load, which re-seeds the mock state.

## Adding a test

1. Open the section with `open(...)`, add the mock switches the case needs.
2. If a screen needs data the mock lacks, add it under `src/dev/mocks/` (never in the spec).
3. `pnpm typecheck` and `pnpm lint` cover this folder.
