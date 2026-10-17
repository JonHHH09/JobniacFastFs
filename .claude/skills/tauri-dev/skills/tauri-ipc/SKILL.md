---
name: tauri-ipc
description: Tauri 2 IPC in practice, from the official docs - Rust commands (`#[tauri::command]`, `tauri::generate_handler!`, `invoke`, async commands, `Result` errors, `tauri::ipc::Response`, `tauri::ipc::Request`), calling the frontend from Rust (`emit`, `emit_to`, `listen`, `unlisten`, `Channel`, `eval`), and managed state (`manage`, `tauri::State`, `Mutex`). Load when adding or debugging a command, passing arguments, returning errors or binary data, streaming progress, emitting or listening to events, or sharing state in `lib.rs`.
---

# Tauri IPC: Commands, Events, Channels, State

These instructions and the files in `references/` are local copies of the official Tauri v2 docs (https://v2.tauri.app/develop/, tauri-docs 4b74da6, 2026-10-13). Work only from them: before writing code or giving advice, read the reference file(s) for the task; cite the page URL from the file's Source header. If the references don't cover something, say so and point to the live page instead of filling the gap from memory.

## Project context
- `src-tauri/src/lib.rs` holds `run()` with the single `tauri::generate_handler![...]`; `main.rs` only calls it. Every new command must be added to that one list.
- Capabilities live in `src-tauri/capabilities/default.json` (see `tauri-security`).
- The React frontend in `src/` calls `invoke` from `@tauri-apps/api/core`; events use `@tauri-apps/api/event`.
- pnpm project: install JS packages with pnpm.

## Key rules

### Commands
- Annotate a function with `#[tauri::command]`; command names must be unique, even across modules (the module path is ignored by `invoke`). (calling-rust.md)
- Commands defined in `lib.rs` must NOT be `pub` (glue-code error `__cmd__command_name is defined multiple times`); commands in a separate module should be `pub` and listed as `commands::name`. (calling-rust.md)
- Register with `.invoke_handler(tauri::generate_handler![a, b])`. Calling `invoke_handler` several times keeps only the last call; pass all commands to one `generate_handler!`. (calling-rust.md)
- Arguments are a JSON object with camelCase keys (`invoke_message` -> `{ invokeMessage }`). `#[tauri::command(rename_all = "snake_case")]` switches to snake_case keys. (calling-rust.md)
- Arguments must implement `serde::Deserialize`; return values (and errors) must implement `serde::Serialize`. (calling-rust.md)
- `invoke` returns a promise. Returning `Err` rejects it, `Ok` resolves it. (calling-rust.md)
- Most error types do not implement `Serialize`: use `map_err(|e| e.to_string())`, or a custom error type (docs use `thiserror`) with a manual `impl serde::Serialize`. (calling-rust.md)
- A tagged error enum (`#[serde(tag = "kind", content = "message")]`) gives the frontend `{ kind, message }` objects. (calling-rust.md)
- Large payloads: `Serialize` returns go through JSON and can be slow; return `tauri::ipc::Response` to send array buffers efficiently. (calling-rust.md)
- Raw request: take `tauri::ipc::Request`, match `request.body()` against `InvokeBody::Raw`, read `request.headers()`. Frontend passes `ArrayBuffer`/`Uint8Array` as payload and `{ headers }` as the third `invoke` argument. (calling-rust.md)
- Rust-based (WASM) frontends need a separate `invoke` binding without args, since Rust has no optional args. (calling-rust.md)
- Without `window.__TAURI__`: the global script needs `app.withGlobalTauri` set to `true` in `tauri.conf.json`; with the npm package use `import { invoke } from '@tauri-apps/api/core'`. (calling-rust.md)

### Async commands
- Async is preferred for heavy work, to avoid UI freezes. `async fn` commands run via `async_runtime::spawn`; non-async commands run on the main thread unless `#[tauri::command(async)]`. (calling-rust.md)
- Async commands cannot take borrowed arguments such as `&str` or `State<'_, T>` unless they return a `Result`; alternatively use owned types like `String` (does not work for `State`). (calling-rust.md)

### Injected parameters
- A command can take `tauri::WebviewWindow` (the calling window; `.label()`), `tauri::AppHandle`, and `tauri::State<T>`. (calling-rust.md)
- `AppHandle`/`WebviewWindow` default to the `Wry` runtime; for another runtime (e.g. mock runtime) make the command generic over `R: Runtime`. (calling-rust.md)

### Events
- Events: no strong typing, always async, cannot return values, JSON payloads only. Not designed for low latency or high throughput; not covered by capabilities for fine-grained control. (calling-rust.md, calling-frontend.md)
- Rust emits with `Emitter` (`emit`, `emit_to`, `emit_filter`); `AppHandle` and `WebviewWindow` implement `Listener` and `Emitter`. Payloads must be `Serialize + Clone`. (calling-frontend.md)
- Global events reach all listeners; webview-specific events (`emit_to`/`emitTo`) are not delivered to ordinary global listeners. Catch-all: `listen_any` in Rust, `{ target: { kind: 'Any' } }` option in JS `listen`. (calling-frontend.md, calling-rust.md)
- Frontend emits with `emit(name, payload)` / `emitTo(label, name, payload)` from `@tauri-apps/api/event`; frontend-emitted events also trigger listeners. (calling-rust.md, calling-frontend.md)
- JS `listen` returns a Promise of `unlisten`: `await` it, or call `unlisten.then(fn => fn())`. Calling it before resolution does not clean up. (calling-frontend.md)
- Always unlisten when the component unmounts; SPA router navigation does not auto-unregister listeners. In React register in `useEffect` and return a cleanup. (calling-frontend.md)
- Async listeners may process rapid events out of order; use Channels for ordered, high-throughput data. (calling-frontend.md)
- `once` listens exactly once. In Rust, `app.listen` returns an id for `app.unlisten(id)`. (calling-frontend.md)

### Channels and eval
- Channels are the recommended mechanism for streaming (download progress, HTTP chunks): fast and ordered. (calling-rust.md, calling-frontend.md)
- Rust: command parameter `on_event: tauri::ipc::Channel<T>` and `on_event.send(...)`. JS: `new Channel<T>()`, set `onmessage`, pass it in the `invoke` args under the matching camelCase key (`onEvent`). (calling-frontend.md)
- `WebviewWindow#eval` runs JS in the webview; for scripts built from Rust data the docs recommend the `serialize-to-javascript` crate. (calling-frontend.md)

### State
- Register with `Builder::manage(...)` or `app.manage(...)` in `setup` (needs `Manager`); read with `State<'_, T>` in commands or `app.state::<T>()`. (state-management.md, calling-rust.md)
- Mutable state: wrap in `std::sync::Mutex` (`manage(Mutex::new(AppState::default()))`). A std `Mutex` is often fine in async code; use Tokio's async mutex only when holding the guard across `.await`. (state-management.md)
- No `Arc` needed for state in `State`; Tauri does it. To move state into a thread, move an `AppHandle` (cheap to clone) and call `.state()` there. (state-management.md)
- Wrong `State` type is a runtime error, not a compile error (`state not managed for field ...`; `Manager::state()` panics). Using a type alias (`type AppState = Mutex<AppStateInner>`) helps; do not wrap the alias in `Mutex` again. (state-management.md)
- Async command taking `State<'_, T>` must return `Result` (e.g. `Result<u32, ()>`). (state-management.md)

## How to add a command end to end
1. Write `#[tauri::command] fn name(args) -> Result<T, E>` (not `pub` in `lib.rs`; `pub` in a module). `E` must implement `Serialize`.
2. Add it to the single `tauri::generate_handler![...]` in `run()`.
3. If it uses state, `manage` it on the builder and take `State<'_, Mutex<T>>`.
4. Call `invoke('name', { camelCaseArg })` from `@tauri-apps/api/core`; handle rejection with `.catch`.
5. Check capability/ACL needs with `tauri-security`. (calling-rust.md)

## How to stream data Rust to frontend
1. Add `on_event: Channel<MyEvent>` to the command; `MyEvent` derives `Clone, Serialize` (docs use `#[serde(tag = "event", content = "data")]`).
2. Call `on_event.send(...)` per chunk.
3. In JS create `new Channel<MyEvent>()`, set `onmessage`, pass as `onEvent` in `invoke`. (calling-frontend.md)

## How to share state between commands
1. `.manage(Mutex::new(AppState::default()))` on the builder.
2. `state: State<'_, Mutex<AppState>>` in each command; `state.lock().unwrap()` for mutation.
3. Outside commands use `app_handle.state::<Mutex<AppState>>()`. (state-management.md)

## Reference index
| File | Read it when |
|---|---|
| [calling-rust.md](references/calling-rust.md) | Writing/registering commands, args, errors, async, raw requests, frontend event API |
| [calling-frontend.md](references/calling-frontend.md) | Emitting/listening to events from Rust, Channels, `eval`, listener cleanup pitfalls |
| [state-management.md](references/state-management.md) | Using `manage`, `State`, `Mutex`, accessing state outside commands |

## Related skills
- `tauri-architecture`: why IPC works this way, Isolation pattern.
- `tauri-security`: capabilities, permissions, scopes, CSP.
- `tauri-testing`: mocking IPC, WebDriver.
- `tauri-plugin-development`, `tauri-official-plugins`, `tauri-develop`, `tauri-distribute`.
