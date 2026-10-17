---
name: tauri-official-plugins
description: "Reference for the official Tauri v2 plugins (autostart, barcode-scanner, biometric, cli, clipboard-manager, deep-link, dialog, fs, geolocation, global-shortcut, haptics, http, localhost, log, nfc, notification, opener, os, persisted-scope, positioner, process, shell, single-instance, sql, store, stronghold, updater, upload, websocket, window-state) with setup, platform limits and permission identifiers. Load when adding, using or configuring any @tauri-apps/plugin-* or tauri-plugin-* package, running `tauri add <plugin>`, registering a plugin in lib.rs, or granting plugin permissions such as fs:default, dialog:allow-open, http:default, shell:allow-execute, store:default, sql:allow-execute or updater:default in a capabilities file."
---

# Official Tauri plugins

These instructions and the files in `references/` are local copies of the official Tauri v2 docs (https://v2.tauri.app/plugin/, tauri-docs 4b74da6, 2026-10-13) plus each plugin's generated permission reference. Work only from them: before adding or using a plugin, read its reference file, including its Permission reference, and cite the page URL from the file's Source header. If the references do not cover something, say so and point to the live page instead of filling the gap from memory.

All paths below are relative to `references/`. Tags like `(file-system.md)` name the file a statement comes from.

## Project context

- Package manager is pnpm: use `pnpm tauri add <plugin>` and `pnpm add @tauri-apps/plugin-<name>` (the pnpm variants shown in each file).
- Rust side lives in `src-tauri/`; plugins are registered in `src-tauri/src/lib.rs` on `tauri::Builder` with `.plugin(...)`.
- Capabilities live in `src-tauri/capabilities/default.json`; plugin permissions are granted there.
- `tauri-plugin-opener` is already registered (`.plugin(tauri_plugin_opener::init())`) and is the plugin for opening files/URLs (opener.md). The shell plugin points to Opener for the `shell.open` API (shell.md).
- Frontend is React 19 + TypeScript + Vite in `src/`; JS bindings are imported from `@tauri-apps/plugin-<name>`.

## Adding a plugin (general procedure)

1. Read `<plugin>.md` fully, including "Supported Platforms" (the table is only on the live page, linked from the file) and the Permission reference.
2. Install, either way (every plugin file):
   - Automatic: `pnpm tauri add <name>` (the name used by `tauri add` is the first column of the catalog below). (opener.md, dialog.md, store.md)
   - Manual: (a) in `src-tauri` run `cargo add tauri-plugin-<name>`; (b) register it in `lib.rs` with `.plugin(...)`; (c) install the JS bindings with `pnpm add @tauri-apps/plugin-<name>` if you use the JS API. (dialog.md, http-client.md, sql.md)
3. Registration forms differ per plugin: `init()` for most (e.g. `tauri_plugin_dialog::init()`), `Builder::new().build()` for store, log, global-shortcut, updater (desktop `setup`), `Builder::default().build()` for sql. Copy the exact call from the plugin's file. (dialog.md, store.md, sql.md, logging.md)
4. Desktop-only plugins are added with a target-gated dependency and registered inside `#[cfg(desktop)]`; mobile-only ones use `cfg(mobile)`. Copy the exact `cargo add ... --target '...'` line and `cfg` guard from the file. (updater.md, autostart.md, biometric.md)
5. Grant permissions in the capability file using the identifiers from the plugin's Permission reference, e.g. `"store:default"` or `{"identifier": "opener:allow-open-url", "allow": [{"url": "https://tauri.app"}]}`. By default all potentially dangerous plugin commands and scopes are blocked. (store.md, opener.md)
6. Least privilege: start from the narrowest `allow-*` identifiers; when a permission takes scopes (fs, http, opener, shell) list only the needed paths/URLs/commands, and use `deny` for exceptions. (opener.md, http-client.md, shell.md, file-system.md)
7. Mobile-only plugins (barcode-scanner, geolocation, haptics) show their permissions in `src-tauri/capabilities/mobile.json` with `"platforms": ["iOS", "android"]`, not in `default.json`. (barcode-scanner.md, geolocation.md, haptics.md)

## Plugin catalog

Plugin = argument of `tauri add` and suffix of the crate `tauri-plugin-<name>` (e.g. `tauri add fs`, `cargo add tauri-plugin-fs`). Permission identifiers use the same name as prefix (e.g. `fs:`, `os:`, `http:`, `log:`, `deep-link:`, `clipboard-manager:`).

| Plugin | Reference | What it does | Platforms / notable limits | Default permission set |
|---|---|---|---|---|
| autostart | autostart.md | Launch app at system startup | Desktop (target-gated dep, `cfg(desktop)`); `init(MacosLauncher::LaunchAgent, Some(args))` | allow-enable, allow-disable, allow-is-enabled |
| barcode-scanner | barcode-scanner.md | Scan QR/EAN-13/other barcodes with camera | Mobile only (android/ios target, `cfg(mobile)`); iOS needs `NSCameraUsageDescription` | all barcode features: allow-cancel, check-permissions, open-app-settings, request-permissions, scan, vibrate |
| biometric | biometric.md | Biometric authentication | Android and iOS only; iOS needs `NSFaceIDUsageDescription` | allow-authenticate, allow-status; `biometric_cipher` not included (needs `biometric:allow-biometric-cipher`) |
| cli | cli.md | Parse CLI args via clap, defined in `tauri.conf.json` | Desktop (target-gated dep) | allow-cli-matches |
| clipboard-manager | clipboard.md | Read/write system clipboard | See file | None; read/write must be enabled explicitly (allow-read-text, allow-write-text, allow-clear, ...) |
| deep-link | deep-linking.md | Make the app the handler for URLs/schemes | Mobile (app links, custom schemes) and desktop (schemes); see file | allow-get-current |
| dialog | dialog.md | Native open/save/message/ask/confirm dialogs | See file | allow-message, allow-save, allow-open |
| fs | file-system.md | Frontend file system access | See file; Android/iOS config notes | create-app-specific-dirs, read-app-specific-dirs-recursive, deny-default (read in app dirs; webview data denied) |
| geolocation | geolocation.md | Current position / watch position | Mobile only; iOS Info.plist text, Android perms auto-added | No default set listed; grant check-permissions, request-permissions, get-current-position, watch-position |
| global-shortcut | global-shortcut.md | Register global shortcuts | Desktop (target-gated dep) | None; enable allow-is-registered, allow-register, allow-unregister explicitly |
| haptics | haptics.md | Haptic feedback / vibration | Android and iOS; may not work on cheaper Android phones | No default set listed; grant allow-impact-feedback, allow-notification-feedback, allow-selection-feedback, allow-vibrate |
| http | http-client.md | HTTP client (fetch in JS, reqwest in Rust) | See file | allow-fetch family, but no origin allowed until you configure a URL scope |
| localhost | localhost.md | Serve assets over a localhost server | Rust only; security risk (caution in file) | Ships no permission reference |
| log | logging.md | Configurable logging | See file | allow-log |
| nfc | nfc.md | Read/write NFC tags | Android and iOS only; iOS 14+ | allow-is-available, allow-scan; writing must be enabled manually |
| notification | notification.md | Native notifications | Actions API mobile only; channels mainly Android | All notification features |
| opener | opener.md | Open files/URLs in external apps, reveal in file explorer | See file | allow-open-url, allow-reveal-item-in-dir, allow-default-urls (mailto, tel, http, https) |
| os | os-info.md | OS information | See file | All info except host name |
| persisted-scope | persisted-scope.md | Persist fs/asset scope changes across restarts | Must be registered after `fs` | Ships no permission reference |
| positioner | positioner.md | Move windows to well-known positions | Desktop (target-gated dep); tray positions need `tray-icon` feature | allow-move-window, allow-move-window-constrained, allow-set-tray-icon-state |
| process | process.md | Exit / restart the app | See file | allow-exit, allow-restart |
| shell | shell.md | Spawn child processes | See file | allow-open only (http(s), tel, mailto links); execution needs scoped allow-execute |
| single-instance | single-instance.md | Allow only one app instance | Desktop (target-gated dep); Linux uses DBus | Ships no permission reference; no JS API, no capability needed |
| sql | sql.md | SQL via sqlx (SQLite, MySQL, PostgreSQL) | Pick driver via Cargo feature | allow-close, allow-load, allow-select (reads only) |
| stronghold | stronghold.md | Encrypted secrets store (IOTA Stronghold) | See file | All non-destructive operations |
| store | store.md | Persistent key-value store | See file | All store operations |
| updater | updater.md | In-app updates (signed) | Desktop (target-gated dep, `cfg(desktop)`) | allow-check, allow-download, allow-install, allow-download-and-install |
| upload | upload.md | HTTP file upload / download | See file | allow-upload, allow-download |
| websocket | websocket.md | WebSocket client in JS (Rust client) | See file | allow-connect, allow-send |
| window-state | window-state.md | Save/restore window size and position | Desktop (target-gated dep) | allow-filename, allow-restore-state, allow-save-window-state |

## Key rules and gotchas

Permissions and scopes
- A permission alone does not grant fs paths: `fs:allow-exists` without an `allow` scope fails at runtime with `forbidden path`; use the object form `{"identifier": "fs:allow-exists", "allow": [{"path": "$HOME/**/*"}]}` or the global `fs:scope`. (file-system.md)
- fs scopes: `deny` takes precedence over `allow`; entries are `{ path }` with `$APPDATA`, `$HOME`, `$APPCONFIG`, etc.; `$APPDATA/*` does not include sub-directories. (file-system.md)
- fs default set only reads app directories (AppConfig, AppData, AppLocalData, AppCache, AppLog) and allows `mkdir` there; webview data (`$APPLOCALDATA/EBWebView` on Windows) is denied. (file-system.md)
- fs dotfiles/dotfolders on Unix need the full path or a glob after the dot component, or `"plugins": {"fs": {"requireLiteralLeadingDot": false}}` in `tauri.conf.json`. (file-system.md)
- fs rejects path traversal (`..`); use a `baseDir` or the path API. Always call `file.close()` after `create`/`open`. (file-system.md)
- fs: the Rust side has no file manipulation API; use `std::fs` or `tokio::fs`. `watch`/`watchImmediate` need the `watch` Cargo feature. (file-system.md)
- http: `http:default` enables fetch but allows no origins; add `allow`/`deny` URL scopes (e.g. `https://*.tauri.app`). Forbidden request headers are ignored unless the `unsafe-headers` Cargo feature is on. (http-client.md)
- opener: `opener:allow-open-path` and `opener:allow-open-url` take `path` / `url` glob scopes; custom protocols must be known to the OS. Default set covers mailto, tel, http, https only. (opener.md)
- shell: spawning needs a scoped `shell:allow-execute` entry with `name`, `cmd`, `args` (fixed strings or `{"validator": "\\S+"}`) and `sidecar`; the default set only allows `open`. (shell.md)
- clipboard-manager and global-shortcut ship no default permissions; enable the exact read/write or register identifiers needed. (clipboard.md, global-shortcut.md)
- dialog: `allow-ask` and `allow-confirm` are deprecated aliases of `allow-message` and go away in v3. Closing a message dialog returns `false`. (dialog.md)
- biometric cipher (`biometric_cipher`) is not in the default set. NFC writing is not in the default set. (biometric.md, nfc.md)

Data and secrets
- store: `load('store.json', { autoSave: false })` options are ignored if that store path already exists; saves on graceful exit, or debounced (100 ms default) when `autoSave` is a number or omitted. `LazyStore` loads on first access; `Store` import is replaced by `LazyStore`. Rust values must be `serde_json::Value`. (store.md)
- sql: enable a driver feature (`--features sqlite`, `mysql` or `postgres`) or the plugin has no engine. (sql.md)
- sql: SQLite and PostgreSQL bind with `$1, $2`, MySQL with `?`. SQLite paths are relative to the AppConfig directory. `sql:default` is read-only; add `sql:allow-execute` for writes. (sql.md)
- sql migrations: unique `version` per migration, registered with `add_migrations("sqlite:db.db", migrations)`; applied by `plugins.sql.preload` in `tauri.conf.json` or by the client `load()`; run in a transaction and should be idempotent. (sql.md)
- stronghold needs a password hash function returning exactly 32 bytes (`Builder::new(|password| ..)` or `Builder::with_argon2(&salt_path)`). Stronghold is the "encrypted, secure database"; store is "persistent key value storage". (stronghold.md, store.md)
- persisted-scope must be registered after `fs`, otherwise persisted scope silently fails (a dev-mode warning is printed). (persisted-scope.md)

Updater and process
- updater: signatures cannot be disabled. Generate keys with `pnpm tauri signer generate -w ~/.tauri/myapp.key`; the public key goes in `plugins.updater.pubkey` (content, not a file path); the private key is supplied via `TAURI_SIGNING_PRIVATE_KEY` (and `_PASSWORD`) environment variables, `.env` files do not work. Losing the private key means you cannot ship updates to installed apps. (updater.md)
- updater config: `bundle.createUpdaterArtifacts: true` (or `"v1Compatible"` when migrating, removed in v3); `endpoints` must be HTTPS in production, next URL is tried only on non-2XX; variables `{{current_version}}`, `{{target}}`, `{{arch}}`. (updater.md)
- updater server: dynamic server answers `204` when no update, else `200` with `version`, `url`, `signature`; static JSON needs `version` and `platforms.<os-arch>.url/signature`, and the whole file is validated before the version check. `signature` is the `.sig` content, not a path or URL. (updater.md)
- updater on Windows: the app exits automatically when the install step runs; use `on_before_exit`; `installMode` is `passive` (default), `basicUi` or `quiet`. (updater.md)
- process: `relaunch()` / `exit()` come from the process plugin; the updater example imports `relaunch` from `@tauri-apps/plugin-process`. (updater.md, process.md)

Registration order and platform
- single-instance must be the first plugin registered. On Linux it uses DBus; snap and flatpak need extra manifest permissions for it. (single-instance.md)
- deep-link with a single-instance setup: add the `deep-link` feature to `tauri-plugin-single-instance`, register single-instance first; validate incoming URLs because a fake deep link can be passed as an argument. (deep-linking.md)
- window-state: the window is restored after creation; create it with `visible: false` and the plugin shows it. (window-state.md)
- positioner: tray-relative positions need the `tray-icon` feature and `on_tray_icon_event` setup; for Rust-only moves only the dependency is needed. (positioner.md)
- log: default targets are stdout and a log-dir file; call `clear_targets` to replace them; to see Rust logs in the webview enable the `Webview` target and call `attachConsole()`; needs the `log` crate in `Cargo.toml`. Grant `log:default`. (logging.md)
- localhost serves assets over a server instead of the custom protocol and carries considerable security risk; prefer the default protocol if in doubt. (localhost.md)
- notification: Actions API is mobile only; create channels before sending notifications that reference them. (notification.md)
- geolocation, haptics, barcode-scanner, biometric, nfc: mobile only; iOS needs the Info.plist usage strings named in each file. (geolocation.md, haptics.md, barcode-scanner.md, biometric.md, nfc.md)

## Related skills

- tauri-security: capabilities, permissions and scopes in depth.
- tauri-ipc: commands, events and channels between frontend and Rust.
- tauri-develop: day-to-day development workflow.
- tauri-plugin-development: writing your own plugin.
- tauri-distribute: releases, updater endpoints and signing.
