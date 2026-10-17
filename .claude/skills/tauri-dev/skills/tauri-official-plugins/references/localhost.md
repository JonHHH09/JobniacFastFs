# Localhost

> Source: https://v2.tauri.app/plugin/localhost/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Use a localhost server in production apps.

Expose your app's assets through a localhost server instead of the default custom protocol.

:::caution
This plugin brings considerable security risks and you should only use it if you know what you are doing. If in doubt, use the default custom protocol implementation.
:::

## Supported Platforms

> Platform support table is generated on the site: https://v2.tauri.app/plugin/localhost/

## Setup

Install the localhost plugin to get started.

**Automatic:**

        Use your project's package manager to add the dependency:

```sh
# npm
npm run tauri add localhost
# yarn
yarn run tauri add localhost
# pnpm
pnpm tauri add localhost
# deno
deno task tauri add localhost
# bun
bun tauri add localhost
# cargo
cargo tauri add localhost
```

**Manual:**

        1.  Run the following command in the `src-tauri` folder to add the plugin to the project's dependencies in `Cargo.toml`:

            ```sh frame=none
            cargo add tauri-plugin-localhost
            ```

        2.  Modify `lib.rs` to initialize the plugin:

            ```rust title="src-tauri/src/lib.rs" ins={4}
            #[cfg_attr(mobile, tauri::mobile_entry_point)]
            pub fn run() {
                tauri::Builder::default()
                    .plugin(tauri_plugin_localhost::Builder::new().build())
                    .run(tauri::generate_context!())
                    .expect("error while running tauri application");
            }
            ```

## Usage

The localhost plugin is available in Rust.

```rust title="src-tauri/src/lib.rs" {4} {7-14}
use tauri::{webview::WebviewWindowBuilder, WebviewUrl};

pub fn run() {
  let port: u16 = 9527;

  tauri::Builder::default()
      .plugin(tauri_plugin_localhost::Builder::new(port).build())
      .setup(move |app| {
          let url = format!("http://localhost:{}", port).parse().unwrap();
          WebviewWindowBuilder::new(app, "main".to_string(), WebviewUrl::External(url))
              .title("Localhost Example")
              .build()?;
          Ok(())
      })
      .run(tauri::generate_context!())
      .expect("error while running tauri application");
}
```

## Permission reference

This plugin ships no permission reference in plugins-workspace.
