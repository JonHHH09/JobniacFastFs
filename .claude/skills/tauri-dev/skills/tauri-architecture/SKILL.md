---
name: tauri-architecture
description: Tauri 2 core concepts from the official docs - architecture and crates (tauri, tauri-runtime, WRY, TAO), the process model (Core process vs WebView process, per-OS webview), the IPC model (Events vs Commands, `invoke`), the Brownfield and Isolation patterns (`app.security.pattern`, `__TAURI_ISOLATION_HOOK__`), and reducing app size (Cargo release profile, `removeUnusedCommands`). Load when explaining how Tauri works, deciding where logic belongs (Rust core vs frontend), configuring the isolation pattern, or shrinking the binary.
---

# Tauri Architecture and Core Concepts

These instructions and the files in `references/` are local copies of the official Tauri v2 docs (https://v2.tauri.app/concept/, tauri-docs 4b74da6, 2026-10-13). Work only from them: before writing code or giving advice, read the reference file(s) for the task; cite the page URL from the file's Source header. If the references don't cover something, say so and point to the live page instead of filling the gap from memory.

## Project context
- Rust backend in `src-tauri/` (`src-tauri/src/lib.rs` holds `run()` with the single `tauri::generate_handler![...]`; `main.rs` only calls it). Config in `src-tauri/tauri.conf.json`, ACL in `src-tauri/capabilities/default.json`.
- Frontend is React 19 + TypeScript + Vite in `src/`; it calls `invoke` from `@tauri-apps/api/core`.
- Package manager is pnpm; Windows dev machine, so the webview is Microsoft Edge WebView2.
- For concrete command/event/state code see the `tauri-ipc` skill.

## Key rules

### Architecture
- Tauri uses the OS webview and ships no runtime; the final binary is compiled from Rust. (architecture.md)
- Tauri directly uses WRY (webview rendering, decides which webview is used) and TAO (window creation, a winit fork with menu bar and system tray). (architecture.md)
- The `tauri` crate reads `tauri.conf.json` (and `Cargo.toml`) at compile time, injects scripts at runtime and hosts the system API. (architecture.md)
- `tauri-codegen` embeds, hashes and compresses assets and parses `tauri.conf.json` at compile time; `tauri-macros` builds the context, handler and command macros. (architecture.md)
- The JS API (`@tauri-apps/api`) is a TypeScript library that uses webview message passing to reach the host. (architecture.md)
- A plugin generally does 3 things: enables Rust code to do something, provides interface glue, provides a JavaScript API. (architecture.md)

### Process model
- Every app has one Core process: the entry point and the only component with full OS access. It creates windows, tray menus and notifications. (process-model.md)
- The Core routes all IPC, so IPC messages can be intercepted, filtered and manipulated in one place. (process-model.md)
- Put global state (settings, database connections) in the Core: it syncs state between windows and keeps sensitive data away from the frontend. (process-model.md)
- The Core does not render UI; it spins up WebView processes using OS libraries. One Core manages one or more WebViews. (process-model.md)
- WebView libraries are dynamically linked, not bundled: smaller app, but platform differences apply like in web development. Windows uses Microsoft Edge WebView2, macOS WKWebView, Linux webkitgtk. (process-model.md)
- Frontend security practice: always sanitize user input, never handle secrets in the frontend, defer as much business logic as possible to the Core. (process-model.md)

### IPC
- IPC is asynchronous message passing of serialized requests/responses; the recipient may reject or discard requests. (inter-process-communication.md)
- Two primitives: Events and Commands. (inter-process-communication.md)
- Events are fire-and-forget, one-way, for lifecycle events and state changes; both the frontend and the Core can emit them. (inter-process-communication.md)
- Events are built on Commands; access to the events API is controlled by the Event permissions. (inter-process-communication.md)
- Commands: `invoke` is similar to browser `fetch`; JSON-RPC-like, so all arguments and return data must be JSON-serializable. (inter-process-communication.md)

### Isolation / Brownfield patterns
- Brownfield is the default pattern, needs no config and has no extra options; explicit form is `app > security > pattern > use: "brownfield"`. (inter-process-communication-brownfield.md)
- Isolation injects a sandboxed `<iframe>` app that intercepts and can modify all IPC messages (including Events) before they reach the Core. Tauri highly recommends it whenever it can be used. (inter-process-communication-isolation.md)
- Isolation hook: set `window.__TAURI_ISOLATION_HOOK__ = (payload) => payload` in the isolation app; return the (possibly modified) payload. (inter-process-communication-isolation.md)
- Config: `app.security.pattern = { "use": "isolation", "options": { "dir": "../dist-isolation" } }`. (inter-process-communication-isolation.md)
- Messages are encrypted with AES-GCM via SubtleCrypto using a key generated on each app start; this adds small overhead vs Brownfield. (inter-process-communication-isolation.md)
- Limitation: external files do not load in sandboxed iframes on Windows, so scripts are inlined at build time. `<script src="index.js">` works, ES Modules do not. (inter-process-communication-isolation.md)
- Keep the isolation app and its dependencies/build steps minimal (supply-chain risk). (inter-process-communication-isolation.md)
- Headless WebDriver runs may need an entropy service such as `haveged` on Linux. (inter-process-communication-isolation.md)

### App size
- Add to `src-tauri/Cargo.toml` (stable): `[profile.dev] incremental = true`; `[profile.release] codegen-units = 1, lto = true, opt-level = "s", panic = "abort", strip = true`. `opt-level` `3` favors speed, `z` size. Stick to stable unless advanced. (size.md)
- Nightly-only extras: `trim-paths = "all"`, `rustflags` (`-Cdebuginfo=0`, `-Zthreads=8`). (size.md)
- `build.removeUnusedCommands: true` in `tauri.conf.json` drops commands never allowed in capability files; to benefit, list only used commands in the ACL instead of `defaults`. (size.md)
- `removeUnusedCommands` needs `tauri@2.4`, `tauri-build@2.1`, `tauri-plugin@2.1`, `tauri-cli@2.4`, and ignores ACLs added dynamically at runtime. (size.md)
- Do not rely on the internal `REMOVE_UNUSED_COMMANDS` env var: it is an implementation detail with no stability guarantee. (size.md)

## How to decide where logic goes
1. Anything needing OS access, secrets, or shared state goes in the Core (Rust); expose it through a command (process-model.md).
2. Use Commands for request/response, Events for fire-and-forget notifications (inter-process-communication.md).
3. Keep the frontend to UI and input handling; sanitize input (process-model.md).

## How to enable the isolation pattern
1. Build an isolation app (an `index.html` plus a script that sets `window.__TAURI_ISOLATION_HOOK__`) into a dir such as `../dist-isolation`.
2. Set `app.security.pattern` to `{ "use": "isolation", "options": { "dir": "../dist-isolation" } }` in `tauri.conf.json`.
3. Avoid ES Modules in the isolation script (Windows limitation). All from inter-process-communication-isolation.md.

## How to shrink the binary
1. Add the release profile to `src-tauri/Cargo.toml` (size.md).
2. Set `build.removeUnusedCommands` to `true` and narrow capability files to the commands you use (size.md).

## Reference index
| File | Read it when |
|---|---|
| [overview.md](references/overview.md) | You need the list of core concept pages |
| [architecture.md](references/architecture.md) | Explaining crates, WRY/TAO, tooling, plugins, license |
| [process-model.md](references/process-model.md) | Deciding Core vs WebView responsibilities, webview per OS |
| [inter-process-communication.md](references/inter-process-communication.md) | Explaining Events vs Commands and the message-passing model |
| [inter-process-communication-brownfield.md](references/inter-process-communication-brownfield.md) | Setting or explaining the default pattern |
| [inter-process-communication-isolation.md](references/inter-process-communication-isolation.md) | Setting up the isolation app, its limits and config |
| [size.md](references/size.md) | Reducing binary size, Cargo profile, `removeUnusedCommands` |

## Related skills
- `tauri-ipc`: writing commands, events, channels, managed state.
- `tauri-security`: capabilities, permissions, scopes, CSP.
- `tauri-develop`: config files, resources, sidecars, icons, debugging, updating deps.
- `tauri-testing`, `tauri-plugin-development`, `tauri-official-plugins`, `tauri-distribute`.
