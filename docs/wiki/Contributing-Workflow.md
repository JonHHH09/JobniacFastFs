# Contributing Workflow

Read the [Code of Conduct](https://github.com/JonHHH09/JobniacFastFs/blob/main/CODE_OF_CONDUCT.md) first. Report vulnerabilities privately as described in [SECURITY.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/SECURITY.md), never in public issues.

Pull requests are open to repository collaborators. To propose a change, open an issue first.

## Branch flow

```text
issue branch --PR (squash, auto-merge)--> development --promotion PR (merge commit, owner)--> main --> release draft
```

- `main` and `development` are protected. Nobody pushes to them directly, the owner included.
- Cut an issue branch from `development` and open the pull request against `development`.
- Auto-merge is switched on for pull requests from a branch of this repository. It squashes the change as soon as the required checks pass, and GitHub deletes the branch.
- `development` is promoted to `main` by a promotion pull request with a merge commit. The owner merges it by hand, because merging into `main` starts the release. See [Release Process](Release-Process.md).
- Keep one concern per pull request. Do not mix unrelated core, UI, MCP, docs or packaging changes.
- Link the issue the change resolves.

## Required checks

A pull request into `development` needs these checks to pass, and the branch must be up to date:

- `Branch policy`
- `UI`
- `Rust (ubuntu-26.04)`
- `Rust (macos-latest)`
- `Supply chain (cargo-deny)`
- `Release tool unit tests`
- `Privacy scan (working tree and history, generic classes)`

Conversations must be resolved. No approvals are required today. See [Testing and CI](Testing-and-CI.md) for what each check does and how skipped jobs count.

## Pull request checklist

The pull request template asks you to confirm:

- The change targets `development`, with one concern.
- Tests are added or updated, and no assertion is weakened.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` pass.
- `pnpm lint`, `pnpm typecheck` and `pnpm test` pass.
- The TypeScript bindings in `src/ipc/bindings.ts` are regenerated if IPC changed.
- Business logic stays in Rust. The UI change only renders or holds UI state.
- Any behaviour change has a `CHANGELOG.md` entry and a test.
- No secrets, tokens, DSNs, local paths or real personal data in code, fixtures or logs.
- Documentation and `CHANGELOG.md` are updated for user-visible changes.
- One concern per commit. Commit messages carry no tool or assistant trailers.

## Commit messages

- Write a short subject in the imperative, for example "Read up to five Arbeitnow pages per scan".
- Use the body to say why, when the subject is not enough.
- No tool or assistant trailers, and no co-author lines for tools.
- One concern per commit.

## Changelog

`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and SemVer. Add your entry under `## [Unreleased]`.

- Every behaviour change and every user-visible change needs an entry.
- MCP tool names, argument keys, envelopes and error semantics are a stable public contract. Any change to them needs an entry in `CHANGELOG.md` and a test. If a change leaves the tools untouched, say so in the entry.
- The release notes of a version are its changelog section. See [Release Process](Release-Process.md).

## Architecture rules

- UI concerns live in TypeScript. Business logic, validation of record, persistence and all IO live in Rust.
- Desktop commands and MCP tools call the same core use cases, never each other.
- File paths, tokens and secrets never cross IPC. `i64` values cross as strings.
- Opt-in features (network fetch, deletion, artifacts, the HTTP listener) stay off unless the exact value `1` is set.

More in [Architecture](Architecture.md).

## Dependencies

Pin new dependencies deliberately and commit the matching `Cargo.lock` and `pnpm-lock.yaml`. A new dependency needs a licence compatible with MIT distribution.

## Licence and personal data

JobniacFast is MIT-licensed. By submitting a contribution you agree that it is licensed under the same terms.

Never commit secrets, tokens, `.env` files, database connection strings, local paths or real personal data such as real resumes, contact details or job-application records. Fixtures and test data must be synthetic. The privacy scan runs in CI and fails on matches. Run it locally before you push:

```text
python3 -I scripts/ci/privacy_scan.py --mode worktree
```

## See also

- [Development Setup](Development-Setup.md)
- [Testing and CI](Testing-and-CI.md)
