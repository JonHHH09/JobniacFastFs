# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

JobniacFast is a Tauri 2 desktop app (Rust core + React 19 / TypeScript / Vite UI) and MCP server over PostgreSQL 18, for managing candidate profiles and job postings, matching them, and generating resume and cover-letter PDFs. Correctness is proven by golden vectors, MCP wire snapshots and PostgreSQL integration tests, never by unit tests alone. MIT-licensed open source: no personal data, secrets, local paths or machine names in tracked files; fixtures are synthetic.

## Project rules

- UI concerns (rendering, UI-only state, animation) live in TypeScript; business logic, validation of record, persistence and all IO live in Rust. Tauri commands are thin adapters over core use cases; desktop commands and MCP tools call the same use cases, never each other.
- File paths, tokens and secrets never cross IPC; `i64` values cross IPC as strings.
- MCP tool names, argument keys, envelopes and error semantics are a stable public contract. Any change to them needs an entry in `CHANGELOG.md` and a test.
- Opt-in features (network fetch, deletion, artifacts, HTTP listener) stay off unless the exact value `1` is set.

## Commands

The package manager is pnpm (`tauri.conf.json` invokes `pnpm dev` / `pnpm build`; `pnpm-lock.yaml` is present).

- `pnpm tauri dev`: run the full desktop app (starts Vite on `http://localhost:1420`, then the Rust shell)
- `pnpm dev`: frontend only, in a browser (Tauri `invoke` calls will not work there)
- `pnpm build`: `tsc && vite build`, type-checks and emits `dist/`
- `pnpm tauri build`: production bundle (runs `pnpm build` first)
- `cargo check` / `cargo build` from `src-tauri/`: Rust only
- CI (`.github/workflows/ci.yml`, Windows + Linux) runs `pnpm build`, then from `src-tauri/`: `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --all-targets --locked`. Run these before pushing. pnpm is pinned via `packageManager` in `package.json`.

## Architecture

Layout (the Cargo workspace members are under `crates/`):

- `src/`: React frontend (`main.tsx` -> `App.tsx`). Calls Rust through `invoke("<command>", args)` from `@tauri-apps/api/core`.
- `src-tauri/src/lib.rs`: the real app entry (`run()`); registers plugins and every `#[tauri::command]` in `tauri::generate_handler![...]`. A new command must be added to that list, or the frontend `invoke` fails. `main.rs` only calls into the lib.
- `src-tauri/Cargo.toml`: the lib is named `jobniacfast_lib` (the `_lib` suffix avoids a Windows bin/lib name clash); edition 2024; release profile uses LTO, `panic = "abort"`, and strip.
- `src-tauri/tauri.conf.json`: window config, dev/build hooks, `frontendDist` is `../dist`. CSP is currently `null`.
- `src-tauri/capabilities/default.json`: Tauri permission set; any new plugin or core API the frontend uses must be granted here.

## Tauri docs plugin

`.claude/skills/tauri-dev/` is a project plugin (loads as `tauri-dev@skills-dir` once the workspace is trusted) with 8 skills: `tauri-architecture`, `tauri-ipc`, `tauri-security`, `tauri-develop`, `tauri-testing`, `tauri-plugin-development`, `tauri-official-plugins`, `tauri-distribute`. Each skill's `references/` holds local Markdown copies of the official v2.tauri.app docs pages (plus each official plugin's generated permission table). Base Tauri work on those references rather than on memory.

- Refresh the references: `py -I .claude/skills/tauri-dev/scripts/sync_docs.py` (clones tauri-docs and plugins-workspace, rewrites every `references/`, leaves `SKILL.md` alone). Review the `references/` diff afterwards and update affected `SKILL.md` rules by hand.
- Don't hand-edit files in `references/`; the next sync overwrites them.
