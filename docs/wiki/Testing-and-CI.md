# Testing and CI

Correctness is proven by golden vectors, MCP wire snapshots and integration tests, not by unit tests alone. All test data is synthetic.

## Test layers

| Layer | Where | Run locally |
|---|---|---|
| Golden vectors | `crates/*/tests/golden/` (text, PDF layout, core domain rules, MCP) | `cargo test --workspace --locked` |
| MCP wire snapshots | `crates/mcp/tests/` (`catalog_snapshot.rs`, `catalog_golden.rs`, `calls_golden.rs`, the HTTP guard tests) | `cargo test -p jobniac-mcp --locked` |
| Integration tests | `crates/*/tests/`, including the CLI, the worker isolation and sandbox tests, and SQLite repositories on a temporary file | `cargo test --workspace --locked` |
| UI unit tests | `src/**` (Vitest) | `pnpm test` |
| Browser UI suite | `playwright/` (Playwright, Chromium, mock IPC, no Rust core) | `pnpm test:ui` |
| End-to-end | `e2e/` (WebDriver through `tauri-driver`, real debug build) | see [e2e/README.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/e2e/README.md) |
| Release tool tests | `scripts/release/tests/` | `python -I -m unittest discover -s scripts/release/tests -p "test_*.py"` |
| CI script tests | `scripts/ci/tests/` | `python3 -I -m unittest discover -s scripts/ci/tests -p "test_*.py"` |

Notes:

- Database tests run in plain `cargo test` against a temporary SQLite file. No database server is needed.
- Tests that read or render PDFs need PDFium. Set `JOBNIAC_PDFIUM_DIR` as described in [Development Setup](Development-Setup.md).
- Platform-specific code needs tests on all three platforms.
- `cargo test` rewrites `src/ipc/bindings.ts`. CI fails when that file changes, so commit the regenerated file when you change IPC types.
- The browser suite needs `pnpm exec playwright install chromium` once. See [playwright/README.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/playwright/README.md).
- The end-to-end suite needs `tauri-driver` and a platform WebDriver. The steps are in its README:

```text
pnpm tauri build --debug --no-bundle --config src-tauri/tauri.e2e.conf.json
cargo build --locked -p jobniac-cli
pnpm e2e
```

## Before you push

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm lint
pnpm typecheck
pnpm test
pnpm build
pnpm test:ui
```

## Privacy scan

`scripts/ci/privacy_scan.py` looks for local paths, e-mail addresses, IP addresses, tokens and keys, database URLs with passwords, phone and street shapes, date-of-birth lines and binary documents. It reads the working tree and every reachable ref.

```text
python3 -I scripts/ci/privacy_scan.py --mode worktree   # working tree only
python3 -I scripts/ci/privacy_scan.py                   # working tree and history
```

On Windows use `py -I`. Exit status `0` means no finding, `1` findings, `2` a usage or configuration error. Reviewed synthetic hits live in `scripts/ci/privacy_allowlist.json`. Never allow-list real personal data. Details are in [scripts/ci/README.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/scripts/ci/README.md).

## What CI runs

Workflow `ci.yml`:

| Job | Runs on | What |
|---|---|---|
| `Changes` | ubuntu-26.04 | Decides which heavy jobs run (see below). |
| `UI` | ubuntu-26.04 | `pnpm lint`, `pnpm typecheck`, `pnpm test`, `pnpm build`, then the Playwright suite. |
| `Rust (ubuntu-26.04)` | ubuntu-26.04 | `cargo fmt --all --check`, clippy with `-D warnings`, `cargo nextest run --workspace --locked`, and the bindings diff check. |
| `Rust (macos-latest)` | macOS, Apple Silicon | The same Rust steps, without the bindings check. Proves the macOS worker sandbox and file handling. |
| `Rust (windows-latest)` | Windows | The same Rust steps. Not run on pull requests into `development`. |
| `Supply chain (cargo-deny)` | ubuntu-26.04 | `cargo deny check`. |
| `Release tool unit tests` | ubuntu-26.04 | The `scripts/release` tests and `release.py check-versions`. |
| `Privacy scan (working tree and history, generic classes)` | ubuntu-26.04 | The scanner tests, then `privacy_scan.py --mode all`. |

Workflow `e2e.yml` runs the WebDriver suite as `E2E (ubuntu-26.04)` and `E2E (windows-latest)`. There is no macOS leg because `tauri-driver` has no macOS driver.

`branch-policy.yml` (`Branch policy`) enforces the source branch: into `main` only from `development`, into `development` only from a branch of this repository. CodeQL and Scorecard run separately.

## Required checks

A pull request into `development` needs `Branch policy`, `UI`, `Rust (ubuntu-26.04)`, `Rust (macos-latest)`, `Supply chain (cargo-deny)`, `Release tool unit tests` and `Privacy scan (working tree and history, generic classes)`. The Windows Rust job and the E2E jobs are not required there. They run on the promotion pull request. A red `E2E (...)` check blocks the release, so do not merge a promotion pull request with one.

## How CI skips repeated work

This is a summary of [docs/ops/branch-protection.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/docs/ops/branch-protection.md).

- A required check whose job is skipped counts as passed. That is why the Rust and E2E jobs are plain jobs with literal names, not a matrix.
- The `Changes` job (`scripts/ci/gate.sh`) path-filters pull requests into `development`. A docs-only change skips `UI`, `Rust (...)` and `E2E (...)`. A Rust-only change skips `UI`.
- The gate fails open. If `Changes` did not succeed, every heavy job runs.
- After a fully green run, `Mark tree as passed` saves a cache entry keyed by the git tree SHA. A later run on the same tree, such as the promotion pull request or the release call, skips the heavy jobs.
- Only markers saved on `development` or `main`, or on a verified `development` to `main` pull request, are trusted. Markers from issue branches and forks are ignored.
- `nightly.yml` runs daily at 02:30 UTC and dispatches `ci.yml` and `e2e.yml` on `development`. This saves the Rust caches and the marker for the `development` tip.
- Pull requests restore Rust caches but never save them.
- A manual run (`workflow_dispatch`) ignores the marker and runs everything.
- Marker entries expire after 7 days without use. A rerun is then full, which is safe.

## See also

- [Development Setup](Development-Setup.md)
- [Contributing Workflow](Contributing-Workflow.md)
- [Release Process](Release-Process.md)
