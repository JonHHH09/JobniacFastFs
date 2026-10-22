# Contributing to JobniacFast

Thanks for your interest. Please read the [Code of Conduct](CODE_OF_CONDUCT.md) first. Report
vulnerabilities privately as described in [SECURITY.md](SECURITY.md), never in public issues.

JobniacFast is a Tauri 2 desktop app (Rust core, React/TypeScript UI) and MCP server.
Read the architecture rules below before proposing a change.

## Development setup

Requirements: Rust (the version pinned by the repository toolchain file, or stable), Node.js 22.12
or later, pnpm, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your
platform.

```text
pnpm install --frozen-lockfile   # install the locked UI dependencies
pnpm tauri dev                   # run the desktop app (Vite dev server + Rust)
pnpm build                       # type-check and build the UI
```

## Checks

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm build
```

- Tests use synthetic data only. Database tests run in plain `cargo test` against a temporary SQLite
  file, never a real or personal database.
- CI runs on Windows, Linux and macOS; platform-specific code needs tests on all three.

## Architecture rules

- UI concerns (rendering, UI state, animation) live in TypeScript; all business logic, validation
  of record, persistence and IO live in Rust. Tauri commands are thin adapters.
- The desktop commands and the MCP tools call the same use cases in the core crate, never each
  other.
- MCP wire names, request keys, envelopes and error semantics are a stable public contract.
- File paths, tokens and secrets never cross the IPC boundary to the UI.
- Any change to that contract is recorded in [CHANGELOG.md](CHANGELOG.md) with a test.

## Branches and commits

- `main` and `development` are protected: no direct pushes, every change goes through a pull
  request with passing checks. See [docs/ops/branch-protection.md](docs/ops/branch-protection.md).
- Create an issue branch from `development` and open the pull request against `development`. It is
  merged by squash and the branch is deleted automatically once it is merged.
- Only `development` is merged into `main` (the release promotion pull request).
- Pull requests are open to repository collaborators. To propose a change, open an issue first.
- Keep one concern per pull request; do not mix unrelated core, UI, MCP, docs or packaging changes.
- Link the issue your change resolves in the pull request.

## Dependencies

Pin new dependencies deliberately and commit the matching `Cargo.lock` and `pnpm-lock.yaml`.
New dependencies must have a licence compatible with MIT distribution.

## Secrets and personal data

Never commit secrets, tokens, `.env` files, database connection strings, local paths or real
personal data (real resumes, contact details, job-application records).

## Licence of contributions

JobniacFast is released under the MIT licence. By submitting a contribution you agree that it is
licensed under the same terms (inbound = outbound).
