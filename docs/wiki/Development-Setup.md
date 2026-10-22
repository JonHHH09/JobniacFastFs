# Development Setup

How to build and run JobniacFast from source.

## Prerequisites

| Tool | Version |
|---|---|
| Rust | The version pinned in [`rust-toolchain.toml`](https://github.com/JonHHH09/JobniacFastFs/blob/main/rust-toolchain.toml), with `rustfmt` and `clippy`. `rustup` installs it on first use. |
| Node.js | 22.12 or later. CI uses Node 24. |
| pnpm | The version in the `packageManager` field of `package.json`. Enable it with Corepack or install that exact version. |
| Tauri prerequisites | The [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS. |
| PDFium | The pinned binary, fetched by script (see below). Needed by the tests that render or read PDFs. |

### Linux

CI installs these packages on Ubuntu:

```text
sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

### Windows and macOS

Follow the Tauri prerequisites page. The project is built and tested on Windows, Linux and macOS (Apple Silicon).

## PDFium

PDFium is fetched into `vendor/pdfium/<platform>/` (gitignored). Each archive and library is checked against a pinned SHA-256.

```text
scripts/fetch-pdfium.sh linux-x64          # Linux, macOS or Git Bash
pwsh -File scripts/fetch-pdfium.ps1 -Platform windows-x64
```

Platforms are `windows-x64`, `linux-x64`, `mac-arm64` and `all` (the default). Then point the environment variable `JOBNIAC_PDFIUM_DIR` at the platform folder, for example `vendor/pdfium/linux-x64`.

## Install and run

```text
pnpm install --frozen-lockfile   # install the locked UI dependencies
pnpm tauri dev                   # full desktop app: Vite on http://localhost:1420, then the Rust shell
pnpm dev                         # UI only, in a browser (Tauri invoke calls do not work there)
pnpm dev:mock                    # UI in a browser against an in-memory mock IPC
pnpm build                       # type-check and build the UI into dist/
pnpm tauri build                 # production bundle (runs pnpm build first)
```

The Rust crates embed the built UI, so run `pnpm build` once before `cargo` commands on a fresh checkout.

## Rust commands

```text
cargo check --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs `cargo nextest run --workspace --locked`, which is equivalent. Run the fmt, clippy and test commands before you push.

## UI commands

```text
pnpm lint            # biome ci .
pnpm typecheck       # tsc --noEmit for src and playwright
pnpm test            # vitest unit tests
pnpm test:ui         # browser UI suite (Playwright)
```

Install the browser once with `pnpm exec playwright install chromium`.

## Notes

- Data lives in an embedded SQLite file. No database server is needed.
- Use synthetic data only. Never use a real resume or a personal database.
- If you change a Tauri command or its types, regenerate `src/ipc/bindings.ts` by running the Rust tests, and commit the result.

## See also

- [Architecture](Architecture.md)
- [Testing and CI](Testing-and-CI.md)
- [Contributing Workflow](Contributing-Workflow.md)
