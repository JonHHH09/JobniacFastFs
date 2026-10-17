---
name: tauri-develop
description: Tauri 2 development workflow - running `tauri dev`, tauri.conf.json and platform configs (tauri.windows.conf.json, --config merge), Cargo.toml/package.json roles, bundling resources (`bundle.resources`, `$RESOURCE`, resolveResource), sidecars (`externalBin`, target-triple suffix, `shell().sidecar`), app icons (`tauri icon`), debugging (RustRover, VS Code, Neovim, devtools feature, CrabNebula, RUST_BACKTRACE, WebKitGTK Linux graphics) and updating @tauri-apps/* and tauri crates. Load for any of these tasks, .taurignore, beforeDevCommand/devUrl, or version-sync questions.
---

# Tauri Develop

These instructions and the files in `references/` are local copies of the official Tauri v2 docs (https://v2.tauri.app/develop/, tauri-docs 4b74da6, 2026-10-13). Work only from them: before writing code or giving advice, read the reference file(s) for the task; cite the page URL from the file's Source header. If the references don't cover something, say so and point to the live page instead of filling the gap from memory.

## Project context
- Run the app with `pnpm tauri dev`; build with `pnpm tauri build` (overview.md).
- `src-tauri/tauri.conf.json`: `build.beforeDevCommand` "pnpm dev", `devUrl` http://localhost:1420, `beforeBuildCommand` "pnpm build", `frontendDist` "../dist".
- Rust lives in `src-tauri/`; frontend in `src/` (React + Vite, pnpm). Developer uses RustRover (see debug-rustrover.md).
- No test suite yet (see the tauri-testing skill).

## Key rules
- `build.devUrl` + `build.beforeDevCommand` point Tauri at the framework dev server; `build.frontendDist` without a dev server makes the CLI serve that folder, which needs an `index.html` (overview.md).
- The built-in Tauri dev server has no mutual authentication or encryption; never use it on untrusted networks (overview.md).
- First `tauri dev` can take several minutes for Rust packages; later builds are cached (overview.md).
- Web Inspector: right-click > Inspect, or `Ctrl + Shift + I` on Windows/Linux (overview.md, debug.md). Windows uses Microsoft Edge DevTools, macOS Safari's, Linux webkit2gtk WebInspector (debug.md).
- `tauri dev` watches `src-tauri` and workspace dependent crates and rebuilds/restarts; disable with `--no-watch`; exclude files with `.taurignore` (gitignore syntax, usually in `src-tauri` or the cargo workspace root) (overview.md).
- Tauri APIs only work inside the app window, so the frontend can't be used in a system browser; `tauri-invoke-http` bridges API calls over HTTP if you want browser devtools (overview.md).
- Commit `src-tauri/Cargo.lock` with `Cargo.toml`; do not commit `src-tauri/target` (overview.md, configuration-files.md).
- Config formats: JSON default; JSON5 or TOML need the `config-json5` / `config-toml` feature on both `tauri` and `tauri-build` in `Cargo.toml`. Field names are case-sensitive; TOML may use kebab-case (configuration-files.md).
- Platform config files `tauri.linux.conf.json`, `tauri.windows.conf.json`, `tauri.macos.conf.json`, `tauri.android.conf.json`, `tauri.ios.conf.json` (or `Tauri.<platform>.toml`) merge into the main config via JSON Merge Patch (RFC 7396) (configuration-files.md).
- In that merge objects merge key by key but arrays are replaced whole (e.g. `bundle.resources`, `app.windows`): omitted fields in a replaced element fall back to defaults, so repeat everything you want to keep (configuration-files.md).
- `--config <json string or file path>` extends config for `dev`, `android dev`, `ios dev`, `build`, `android build`, `ios build`, `bundle`; e.g. a beta flavour with its own `productName` and `identifier` (configuration-files.md).
- `tauri`, `tauri-build` and the Tauri CLI should generally be on the same latest minor versions; `tauri dev`/`tauri build` manage `tauri` features from the config automatically (configuration-files.md).
- Resources: `bundle.resources` as list (path structure preserved under `$RESOURCE`) or map (source -> target). `..` becomes `_up_`, absolute root becomes `_root_` in list form (resources.md).
- Resource globs: `"dir/"` recursive with dirs, `"dir/*"` non-recursive, `"dir/**/*"` recursive files; `"dir/**"` errors. In map form, glob matches are flattened into the target dir (resources.md).
- Resolve resources with `app.path().resolve(path, BaseDirectory::Resource)` (Rust) or `resolveResource` from `@tauri-apps/api/path` (JS); path syntax follows `bundle.resources` (resources.md).
- On Android resources are APK assets with `asset://localhost/` URIs: read via the `fs` plugin, not `std::fs` (resources.md).
- Reading resources from JS via the `fs` plugin needs capability permissions such as `fs:allow-read-text-file` and `fs:allow-resource-read-recursive`; for `_up_`/`_root_` files allow `$RESOURCE/**/*` (resources.md).
- Sidecar: list in `bundle.externalBin` (paths relative to `tauri.conf.json`); a binary named `<name>-$TARGET_TRIPLE` (+ `.exe` on Windows) must exist for each target; get the host triple with `rustc --print host-tuple` (Rust 1.84+, else parse `rustc -Vv`) (sidecar.md).
- Rust `shell().sidecar("name")` takes only the file name, not the `externalBin` path; JS `Command.sidecar(...)` string must match an `externalBin` entry (sidecar.md).
- Sidecar from JS needs `shell:allow-execute` (or `shell:allow-spawn`) with `"sidecar": true` and `name` in `capabilities/default.json`; args are static strings or `{ "validator": "\\S+" }` regexes, `true` allows any, `false` none (sidecar.md).
- Sidecar from Rust requires the shell plugin to be set up first (sidecar.md).
- `tauri icon [INPUT]` default input `./app-icon.png` (squared PNG or SVG with transparency); options `-o/--output`, `-p/--png`, `--ios-color`; desktop icons go to `src-tauri/icons`, mobile icons directly into Xcode/Android projects (icons.md).
- `icon.icns` macOS, `icon.ico` Windows, `*.png` Linux; keep all icons even for a subset of platforms. `bundle.icon` lists the icon files (icons.md).
- Manual icons: `.ico` needs layers 16, 24, 32, 48, 64, 256 (32px first for dev); PNG must be square RGBA 32bpp; iOS icons must have no transparency (icons.md).
- Dev-only code: `#[cfg(dev)]`, `cfg!(dev)`, `tauri::is_dev()` (true under `tauri dev`); `debug_assertions` is true for `tauri dev` and `tauri build --debug` (debug.md).
- Inspector in release needs the `devtools` Cargo feature on `tauri`; it uses a private macOS API that blocks App Store acceptance (debug.md).
- `tauri build --debug` bundles to `src-tauri/target/debug/bundle`; `RUST_BACKTRACE=1` gives stack traces (PowerShell: `$env:RUST_BACKTRACE=1`) (debug.md).
- IDE debugging bypasses the Tauri CLI: build with `--no-default-features` so the dev server is used, and run `beforeDevCommand` yourself (debug-rustrover.md, debug-vscode.md).
- RustRover: if no top-level `Cargo.toml`, attach `src-tauri/Cargo.toml` in the Cargo tool window, or add a root workspace with `members = ["src-tauri"]` (debug-rustrover.md).
- Keep npm `@tauri-apps/api` and crate `tauri` on the same minor; plugin npm packages and crates need exactly the same version (e.g. `@tauri-apps/plugin-fs` and `tauri-plugin-fs`) (updating-dependencies.md).
- Linux/NVIDIA blank window or resize crash: try `__NV_DISABLE_EXPLICIT_SYNC=1`, `WEBKIT_DISABLE_DMABUF_RENDERER=1`, then `WEBKIT_DISABLE_COMPOSITING_MODE=1`; WebGL renderer string is masked so you can't detect slow paths (debug-linux-graphics.md).

## How to change config per platform
1. Put shared settings in `src-tauri/tauri.conf.json`.
2. Add `src-tauri/tauri.windows.conf.json` (etc.) with only the differences (configuration-files.md).
3. Repeat any array you override in full, since arrays are replaced, not merged.
4. For extra flavours use `pnpm tauri build --config src-tauri/tauri.beta.conf.json`.

## How to bundle a resource and read it
1. Add `"bundle": { "resources": ["lang/*"] }` in `tauri.conf.json` (resources.md).
2. Rust: `app.path().resolve("lang/de.json", BaseDirectory::Resource)?` then read the file (resources.md).
3. JS: `resolveResource('lang/de.json')` then `readTextFile` from `@tauri-apps/plugin-fs`, with the fs permissions above.

## How to add a sidecar
1. Add `"externalBin": ["binaries/my-sidecar"]` to `bundle` (sidecar.md).
2. Place `src-tauri/binaries/my-sidecar-<triple>[.exe]` for each target triple.
3. Initialize the shell plugin, grant `shell:allow-execute` with `"sidecar": true` for JS use.
4. Rust: `app.shell().sidecar("my-sidecar")` then `.spawn()`; JS: `Command.sidecar('binaries/my-sidecar')`.

## How to regenerate icons
Run `pnpm tauri icon path/to/app-icon.png` (omit the path to use `./app-icon.png`); output lands in `src-tauri/icons` (icons.md).

## How to debug in RustRover
1. Ensure the Cargo project is loaded (see rules above) (debug-rustrover.md).
2. Create a **Cargo** run configuration with `--no-default-features` added.
3. Create an **npm** run configuration (pnpm, script matching `beforeDevCommand`) for the dev server.
4. Start the dev server, then press **Debug** on the Cargo configuration.

## How to update dependencies
- npm: `pnpm update @tauri-apps/cli @tauri-apps/api --latest`; check with `pnpm outdated @tauri-apps/cli` (updating-dependencies.md).
- Cargo: bump `tauri` and `tauri-build` in `src-tauri/Cargo.toml`, then `cargo update` in `src-tauri` (or `cargo upgrade` from cargo-edit) (updating-dependencies.md).

## Reference index
| File | Read it when |
|---|---|
| [overview.md](references/overview.md) | `tauri dev`, dev server, mobile dev, `.taurignore`, source control |
| [configuration-files.md](references/configuration-files.md) | tauri.conf.json formats, platform configs, `--config`, Cargo.toml, package.json |
| [resources.md](references/resources.md) | `bundle.resources`, resolving/reading resource files, permissions |
| [sidecar.md](references/sidecar.md) | `externalBin`, target triples, running/args of sidecars |
| [icons.md](references/icons.md) | `tauri icon`, manual icon requirements, Android/iOS icons |
| [updating-dependencies.md](references/updating-dependencies.md) | upgrading npm/Cargo Tauri packages, version syncing |
| [debug.md](references/debug.md) | dev-only code, consoles, devtools in production, backtraces |
| [debug-rustrover.md](references/debug-rustrover.md) | debugging the core process in RustRover/JetBrains |
| [debug-vscode.md](references/debug-vscode.md) | launch.json/tasks.json for lldb or cppvsdbg |
| [debug-neovim.md](references/debug-neovim.md) | nvim-dap with codelldb |
| [debug-crabnebula-devtools.md](references/debug-crabnebula-devtools.md) | `tauri-plugin-devtools` instrumentation |
| [debug-linux-graphics.md](references/debug-linux-graphics.md) | blank/flickering window on Linux WebKitGTK |

## Related skills
tauri-architecture, tauri-ipc (commands, events, channels, state), tauri-security (capabilities, permissions, scopes, CSP), tauri-testing, tauri-plugin-development, tauri-official-plugins (fs, shell, opener, ...), tauri-distribute (bundling, signing, CI pipelines).
