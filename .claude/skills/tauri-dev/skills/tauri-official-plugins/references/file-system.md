# File System

> Source: https://v2.tauri.app/plugin/file-system/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Access the file system.

Access the file system.

:::note[Use std::fs or tokio::fs on the Rust side]
If you want to manipulate files/directories through Rust, use traditional Rust's libs (std::fs, tokio::fs, etc).

:::

## Supported Platforms

> Platform support table is generated on the site: https://v2.tauri.app/plugin/file-system/

## Setup

Install the fs plugin to get started.

**Automatic:**

    	Use your project's package manager to add the dependency:

    	{ ' ' }

```sh
# npm
npm run tauri add fs
# yarn
yarn run tauri add fs
# pnpm
pnpm tauri add fs
# deno
deno task tauri add fs
# bun
bun tauri add fs
# cargo
cargo tauri add fs
```

**Manual:**

        1. Run the following command in the `src-tauri` folder to add the plugin to the project's dependencies in `Cargo.toml`:

            ```sh frame=none
            cargo add tauri-plugin-fs
            ```

        2.	Modify `lib.rs` to initialize the plugin:

            ```rust title="src-tauri/src/lib.rs" ins={4}
            #[cfg_attr(mobile, tauri::mobile_entry_point)]
            pub fn run() {
              tauri::Builder::default()
                .plugin(tauri_plugin_fs::init())
                .run(tauri::generate_context!())
                .expect("error while running tauri application");
            }
            ```

        3.	Install the JavaScript Guest bindings using your preferred JavaScript package manager:

```sh
# npm
npm install @tauri-apps/plugin-fs
# yarn
yarn add @tauri-apps/plugin-fs
# pnpm
pnpm add @tauri-apps/plugin-fs
# deno
deno add npm:@tauri-apps/plugin-fs
# bun
bun add @tauri-apps/plugin-fs
```

## Configuration

### Android

When using the audio, cache, documents, downloads, picture, public or video directories your app must have access to the external storage.

Include the following permissions to the `manifest` tag in the `gen/android/app/src/main/AndroidManifest.xml` file:

```xml
<uses-permission android:name="android.permission.READ_EXTERNAL_STORAGE"/>
<uses-permission android:name="android.permission.WRITE_EXTERNAL_STORAGE" />
```

### iOS

Apple requires app developers to specify approved reasons for API usage to enhance user privacy.

You must create a `PrivacyInfo.xcprivacy` file in the `src-tauri/gen/apple` folder
with the required [NSPrivacyAccessedAPICategoryFileTimestamp] key and the [C617.1] recommended reason.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>NSPrivacyAccessedAPITypes</key>
    <array>
      <dict>
        <key>NSPrivacyAccessedAPIType</key>
        <string>NSPrivacyAccessedAPICategoryFileTimestamp</string>
        <key>NSPrivacyAccessedAPITypeReasons</key>
        <array>
          <string>C617.1</string>
        </array>
      </dict>
    </array>
  </dict>
</plist>
```

## Usage

The fs plugin is available in both JavaScript and Rust.
:::caution[Different APIs]
Although this plugin has a file manipulation API on the frontend, in the backend it offers only the methods to change permission of some
resources (files, directories, etc).

In the Rust side you can use the traditional file manipulation libraries,
[std::fs](https://doc.rust-lang.org/std/fs/struct.File.html), [tokio::fs](https://docs.rs/tokio/latest/tokio/fs/index.html) or others.

:::

**JavaScript:**

```javascript
import { exists, BaseDirectory } from '@tauri-apps/plugin-fs';
// when using `"withGlobalTauri": true`, you may use
// const { exists, BaseDirectory } = window.__TAURI__.fs;

// Check if the `$APPDATA/avatar.png` file exists
await exists('avatar.png', { baseDir: BaseDirectory.AppData });
```

**Rust:**

```rust title="src-tauri/src/lib.rs"
use tauri_plugin_fs::FsExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
      .plugin(tauri_plugin_fs::init())
      .setup(|app| {
          // allowed the given directory
          let scope = app.fs_scope();
        	scope.allow_directory("/path/to/directory", false);
          dbg!(scope.allowed());

          Ok(())
       })
       .run(tauri::generate_context!())
       .expect("error while running tauri application");
}
```

## Security

This module prevents path traversal, not allowing parent directory accessors to be used
(i.e. "/usr/path/to/../file" or "../path/to/file" paths are not allowed).
Paths accessed with this API must be either relative to one of the [base directories][base directory]
or created with the [path API].

See [@tauri-apps/plugin-fs - Security] for more information.

## Paths

The file system plugin offers two ways of manipulating paths: the [base directory] and the [path API].

- base directory

  Every API has an options argument that lets you define a [baseDir][base directory] that acts as the working directory of the operation.

  ```js
  import { readFile } from '@tauri-apps/plugin-fs';
  const contents = await readFile('avatars/tauri.png', {
    baseDir: BaseDirectory.Home,
  });
  ```

  In the above example the ~/avatars/tauri.png file is read since we are using the **Home** base directory.

- path API

  Alternatively you can use the path APIs to perform path manipulations.

  ```js
  import { readFile } from '@tauri-apps/plugin-fs';
  import * as path from '@tauri-apps/api/path';
  const home = await path.homeDir();
  const contents = await readFile(await path.join(home, 'avatars/tauri.png'));
  ```

## Files

### Create

Creates a file and returns a handle to it. If the file already exists, it is truncated.

```js
import { create, BaseDirectory } from '@tauri-apps/plugin-fs';
const file = await create('foo/bar.txt', { baseDir: BaseDirectory.AppData });
await file.write(new TextEncoder().encode('Hello world'));
await file.close();
```

:::note
Always call `file.close()` when you are done manipulating the file.
:::

### Write

The plugin offers separate APIs for writing text and binary files for performance.

- text files

  ```js
  import { writeTextFile, BaseDirectory } from '@tauri-apps/plugin-fs';
  const contents = JSON.stringify({ notifications: true });
  await writeTextFile('config.json', contents, {
    baseDir: BaseDirectory.AppConfig,
  });
  ```

- binary files

  ```js
  import { writeFile, BaseDirectory } from '@tauri-apps/plugin-fs';
  const contents = new Uint8Array(); // fill a byte array
  await writeFile('config', contents, {
    baseDir: BaseDirectory.AppConfig,
  });
  ```

### Open

Opens a file and returns a handle to it.
With this API you have more control over how the file should be opened
(read-only mode, write-only mode, append instead of overwrite, only create if it does not exist, etc).

:::note
Always call `file.close()` when you are done manipulating the file.
:::

- read-only

  This is the default mode.

  ```js
  import { open, BaseDirectory } from '@tauri-apps/plugin-fs';
  const file = await open('foo/bar.txt', {
    read: true,
    baseDir: BaseDirectory.AppData,
  });

  const stat = await file.stat();
  const buf = new Uint8Array(stat.size);
  await file.read(buf);
  const textContents = new TextDecoder().decode(buf);
  await file.close();
  ```

- write-only

  ```js
  import { open, BaseDirectory } from '@tauri-apps/plugin-fs';
  const file = await open('foo/bar.txt', {
    write: true,
    baseDir: BaseDirectory.AppData,
  });
  await file.write(new TextEncoder().encode('Hello world'));
  await file.close();
  ```

  By default the file is truncated on any `file.write()` call.
  See the following example to learn how to append to the existing contents instead.

- append

  ```js
  import { open, BaseDirectory } from '@tauri-apps/plugin-fs';
  const file = await open('foo/bar.txt', {
    append: true,
    baseDir: BaseDirectory.AppData,
  });
  await file.write(new TextEncoder().encode('world'));
  await file.close();
  ```

  Note that `{ append: true }` has the same effect as `{ write: true, append: true }`.

- truncate

  When the `truncate` option is set and the file already exists, it will be truncated to length 0.

  ```js
  import { open, BaseDirectory } from '@tauri-apps/plugin-fs';
  const file = await open('foo/bar.txt', {
    write: true,
    truncate: true,
    baseDir: BaseDirectory.AppData,
  });
  await file.write(new TextEncoder().encode('world'));
  await file.close();
  ```

  This option requires `write` to be `true`.

  You can use it along the `append` option if you want to rewrite an existing file using multiple `file.write()` calls.

- create

  By default the `open` API only opens existing files. To create the file if it does not exist,
  opening it if it does, set `create` to `true`:

  ```js
  import { open, BaseDirectory } from '@tauri-apps/plugin-fs';
  const file = await open('foo/bar.txt', {
    write: true,
    create: true,
    baseDir: BaseDirectory.AppData,
  });
  await file.write(new TextEncoder().encode('world'));
  await file.close();
  ```

  In order for the file to be created, `write` or `append` must also be set to `true`.

  To fail if the file already exists, see `createNew`.

- createNew

  `createNew` works similarly to `create`, but will fail if the file already exists.

  ```js
  import { open, BaseDirectory } from '@tauri-apps/plugin-fs';
  const file = await open('foo/bar.txt', {
    write: true,
    createNew: true,
    baseDir: BaseDirectory.AppData,
  });
  await file.write(new TextEncoder().encode('world'));
  await file.close();
  ```

  In order for the file to be created, `write` must also be set to `true`.

### Read

The plugin offers separate APIs for reading text and binary files for performance.

- text files

  ```js
  import { readTextFile, BaseDirectory } from '@tauri-apps/plugin-fs';
  const configToml = await readTextFile('config.toml', {
    baseDir: BaseDirectory.AppConfig,
  });
  ```

  If the file is large you can stream its lines with the `readTextFileLines` API:

  ```typescript
  import { readTextFileLines, BaseDirectory } from '@tauri-apps/plugin-fs';
  const lines = await readTextFileLines('app.logs', {
    baseDir: BaseDirectory.AppLog,
  });
  for await (const line of lines) {
    console.log(line);
  }
  ```

- binary files

  ```js
  import { readFile, BaseDirectory } from '@tauri-apps/plugin-fs';
  const icon = await readFile('icon.png', {
    baseDir: BaseDirectory.Resources,
  });
  ```

### Remove

Call `remove()` to delete a file. If the file does not exist, an error is returned.

```js
import { remove, BaseDirectory } from '@tauri-apps/plugin-fs';
await remove('user.db', { baseDir: BaseDirectory.AppLocalData });
```

### Copy

The `copyFile` function takes the source and destination paths.
Note that you must configure each base directory separately.

```js
import { copyFile, BaseDirectory } from '@tauri-apps/plugin-fs';
await copyFile('user.db', 'user.db.bk', {
  fromPathBaseDir: BaseDirectory.AppLocalData,
  toPathBaseDir: BaseDirectory.Temp,
});
```

In the above example the \<app-local-data\>/user.db file is copied to $TMPDIR/user.db.bk.

### Exists

Use the `exists()` function to check if a file exists:

```js
import { exists, BaseDirectory } from '@tauri-apps/plugin-fs';
const tokenExists = await exists('token', {
  baseDir: BaseDirectory.AppLocalData,
});
```

### Metadata

File metadata can be retrieved with the `stat` and the `lstat` functions.
`stat` follows symlinks (and returns an error if the actual file it points to is not allowed by the scope)
and `lstat` does not follow symlinks, returning the information of the symlink itself.

```js
import { stat, BaseDirectory } from '@tauri-apps/plugin-fs';
const metadata = await stat('app.db', {
  baseDir: BaseDirectory.AppLocalData,
});
```

### Rename

The `rename` function takes the source and destination paths.
Note that you must configure each base directory separately.

```js
import { rename, BaseDirectory } from '@tauri-apps/plugin-fs';
await rename('user.db.bk', 'user.db', {
  fromPathBaseDir: BaseDirectory.AppLocalData,
  toPathBaseDir: BaseDirectory.Temp,
});
```

In the above example the \<app-local-data\>/user.db.bk file is renamed to $TMPDIR/user.db.

### Truncate

Truncates or extends the specified file to reach the specified file length (defaults to 0).

- truncate to 0 length

```typescript
import { truncate } from '@tauri-apps/plugin-fs';
await truncate('my_file.txt', 0, { baseDir: BaseDirectory.AppLocalData });
```

- truncate to a specific length

```typescript
import {
  truncate,
  readTextFile,
  writeTextFile,
  BaseDirectory,
} from '@tauri-apps/plugin-fs';

const filePath = 'file.txt';
await writeTextFile(filePath, 'Hello World', {
  baseDir: BaseDirectory.AppLocalData,
});
await truncate(filePath, 7, {
  baseDir: BaseDirectory.AppLocalData,
});
const data = await readTextFile(filePath, {
  baseDir: BaseDirectory.AppLocalData,
});
console.log(data); // "Hello W"
```

## Directories

### Create

To create a directory, call the `mkdir` function:

```js
import { mkdir, BaseDirectory } from '@tauri-apps/plugin-fs';
await mkdir('images', {
  baseDir: BaseDirectory.AppLocalData,
});
```

### Read

The `readDir` function recursively lists the entries of a directory:

```typescript
import { readDir, BaseDirectory } from '@tauri-apps/plugin-fs';
const entries = await readDir('users', { baseDir: BaseDirectory.AppLocalData });
```

### Remove

Call `remove()` to delete a directory. If the directory does not exist, an error is returned.

```js
import { remove, BaseDirectory } from '@tauri-apps/plugin-fs';
await remove('images', { baseDir: BaseDirectory.AppLocalData });
```

If the directory is not empty, the `recursive` option must be set to `true`:

```js
import { remove, BaseDirectory } from '@tauri-apps/plugin-fs';
await remove('images', {
  baseDir: BaseDirectory.AppLocalData,
  recursive: true,
});
```

### Exists

Use the `exists()` function to check if a directory exists:

```js
import { exists, BaseDirectory } from '@tauri-apps/plugin-fs';
const tokenExists = await exists('images', {
  baseDir: BaseDirectory.AppLocalData,
});
```

### Metadata

Directory metadata can be retrieved with the `stat` and the `lstat` functions.
`stat` follows symlinks (and returns an error if the actual file it points to is not allowed by the scope)
and `lstat` does not follow symlinks, returning the information of the symlink itself.

```js
import { stat, BaseDirectory } from '@tauri-apps/plugin-fs';
const metadata = await stat('databases', {
  baseDir: BaseDirectory.AppLocalData,
});
```

## Watching changes

To watch a directory or file for changes, use the `watch` or `watchImmediate` functions.

- watch

  `watch` is debounced so it only emits events after a certain delay:

  ```js
  import { watch, BaseDirectory } from '@tauri-apps/plugin-fs';
  await watch(
    'app.log',
    (event) => {
      console.log('app.log event', event);
    },
    {
      baseDir: BaseDirectory.AppLog,
      delayMs: 500,
    }
  );
  ```

- watchImmediate

  `watchImmediate` immediately notifies listeners of an event:

  ```js
  import { watchImmediate, BaseDirectory } from '@tauri-apps/plugin-fs';
  await watchImmediate(
    'logs',
    (event) => {
      console.log('logs directory event', event);
    },
    {
      baseDir: BaseDirectory.AppLog,
      recursive: true,
    }
  );
  ```

By default watch operations on a directory are not recursive.
Set the `recursive` option to `true` to recursively watch for changes on all sub-directories.

:::note
The watch functions require the `watch` feature flag:

```toml title="src-tauri/Cargo.toml"
[dependencies]
tauri-plugin-fs = { version = "2.0.0", features = ["watch"] }
```

:::

## Permissions

By default all potentially dangerous plugin commands and scopes are blocked and cannot be accessed. You must modify the permissions in your `capabilities` configuration to enable these.

See the [Capabilities Overview](https://v2.tauri.app/security/capabilities/) for more information and the [step by step guide](https://v2.tauri.app/learn/security/using-plugin-permissions/) to use plugin permissions.

:::caution[Permissions alone do not grant a scope]
Enabling a permission such as `fs:allow-exists` by itself does **not** allow access to any path. Most `fs` commands also require a **scope** that explicitly lists which paths the command may access. Without an `allow` scope, calls will fail at runtime with a `forbidden path` error, even though the permission is enabled.

```json title="Not enough — permission has no scope, so no paths are allowed"
{
  "permissions": ["fs:default", "fs:allow-exists"]
}
```

```json title="Works — the identifier form adds an explicit allow scope"
{
  "permissions": [
    "fs:default",
    {
      "identifier": "fs:allow-exists",
      "allow": [{ "path": "$HOME" }, { "path": "$HOME/**/*" }]
    }
  ]
}
```

See [Scopes](#scopes) below for the full list of path variables you can use in `allow` and `deny`.
:::

```json title="src-tauri/capabilities/default.json" ins={7-11}
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "main-capability",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": [
    "fs:default",
    {
      "identifier": "fs:allow-exists",
      "allow": [{ "path": "$APPDATA/*" }]
    }
  ]
}
```

See **Permission reference** at the end of this file (generated from the plugin's `permissions/autogenerated/reference.md`).

### Scopes

This plugin permissions includes scopes for defining which paths are allowed or explicitly rejected.
For more information on scopes, see the [Command Scopes].

Each `allow` or `deny` scope must include an array listing all paths that should be allowed or denied.
The scope entries are in the `{ path: string }` format.

:::note
`deny` take precedence over `allow` so if a path is denied by a scope, it will be blocked at runtime
even if it is allowed by another scope.
:::

Scope entries can use `$<path>` variables to reference common system paths such as the home directory,
the app resources directory and the config directory. The following table lists all common paths you can reference:

| Path                                                                        | Variable      |
| --------------------------------------------------------------------------- | ------------- |
| [appConfigDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#appconfigdir)       | $APPCONFIG    |
| [appDataDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#appdatadir)           | $APPDATA      |
| [appLocalDataDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#applocaldatadir) | $APPLOCALDATA |
| [appcacheDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#appcachedir)         | $APPCACHE     |
| [applogDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#applogdir)             | $APPLOG       |
| [audioDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#audiodir)               | $AUDIO        |
| [cacheDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#cachedir)               | $CACHE        |
| [configDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#configdir)             | $CONFIG       |
| [dataDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#datadir)                 | $DATA         |
| [localDataDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#localdatadir)       | $LOCALDATA    |
| [desktopDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#desktopdir)           | $DESKTOP      |
| [documentDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#documentdir)         | $DOCUMENT     |
| [downloadDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#downloaddir)         | $DOWNLOAD     |
| [executableDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#executabledir)     | $EXE          |
| [fontDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#fontdir)                 | $FONT         |
| [homeDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#homedir)                 | $HOME         |
| [pictureDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#picturedir)           | $PICTURE      |
| [publicDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#publicdir)             | $PUBLIC       |
| [runtimeDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#runtimedir)           | $RUNTIME      |
| [templateDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#templatedir)         | $TEMPLATE     |
| [videoDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#videodir)               | $VIDEO        |
| [resourceDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#resourcedir)         | $RESOURCE     |
| [tempDir](https://v2.tauri.app/reference/javascript/api/namespacepath/#tempdir)                 | $TEMP         |

#### Examples

- global scope

To apply a scope to any `fs` command, use the `fs:scope` permission:

```json title="src-tauri/capabilities/default.json" {7-10}
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "main-capability",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": [
    {
      "identifier": "fs:scope",
      "allow": [{ "path": "$APPDATA" }, { "path": "$APPDATA/**/*" }]
    }
  ]
}
```

To apply a scope to a specific `fs` command,
use the object form of permissions `{ "identifier": string, "allow"?: [], "deny"?: [] }`:

```json title="src-tauri/capabilities/default.json" {7-18}
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "main-capability",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": [
    {
      "identifier": "fs:allow-rename",
      "allow": [{ "path": "$HOME/**/*" }]
    },
    {
      "identifier": "fs:allow-rename",
      "deny": [{ "path": "$HOME/.config/**/*" }]
    },
    {
      "identifier": "fs:allow-exists",
      "allow": [{ "path": "$APPDATA/*" }]
    }
  ]
}
```

In the above example you can use the [`exists`](#exists) API using any `$APPDATA` sub path (does not include sub-directories)
and the [`rename`](#rename)

:::tip
If you are trying to access dotfiles (e.g. `.gitignore`) or dotfolders (e.g. `.ssh`) on Unix based systems,
then you need to specify either the full path `/home/user/.ssh/example` or the glob after the dotfolder path
component `/home/user/.ssh/*`.

If that does not work in your use case then you can configure the plugin to treat any component
as a valid path literal.

```json title="src-tauri/tauri.conf.json"
 "plugins": {
    "fs": {
      "requireLiteralLeadingDot": false
    }
  }
```

The same option exists for **`app.security.assetProtocol.scope`** when you use the [object form](https://v2.tauri.app/security/asset-protocol/) (not the array-only form). For real-world cases involving dot-directories, see [tauri#13788](https://github.com/tauri-apps/tauri/issues/13788).

:::

[NSPrivacyAccessedAPICategoryFileTimestamp]: https://developer.apple.com/documentation/bundleresources/privacy_manifest_files/describing_use_of_required_reason_api#4278393
[C617.1]: https://developer.apple.com/documentation/bundleresources/privacy_manifest_files/describing_use_of_required_reason_api#4278393
[base directory]: /reference/javascript/api/namespacepath/#basedirectory
[path API]: /reference/javascript/api/namespacepath/
[@tauri-apps/plugin-fs - Security]: /reference/javascript/fs/#security
[Command Scopes]: /security/scope/

## Permission reference

> Source: tauri-apps/plugins-workspace `plugins/fs/permissions/autogenerated/reference.md`

## Default Permission

This set of permissions describes the what kind of
file system access the `fs` plugin has enabled or denied by default.

#### Granted Permissions

This default permission set enables read access to the
application specific directories (AppConfig, AppData, AppLocalData, AppCache,
AppLog) and all files and sub directories created in it.
The location of these directories depends on the operating system,
where the application is run.

In general these directories need to be manually created
by the application at runtime, before accessing files or folders
in it is possible.

Therefore, it is also allowed to create all of these folders via
the `mkdir` command.

#### Denied Permissions

This default permission set prevents access to critical components
of the Tauri application by default.

- On Windows the access to webview data folder `$APPLOCALDATA/EBWebView` is denied.
- On Linux the access to webview data paths inside `$APPLOCALDATA` are denied.

#### This default permission set includes the following:

- `create-app-specific-dirs`
- `read-app-specific-dirs-recursive`
- `deny-default`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`fs:allow-app-read-recursive`

</td>
<td>

This allows full recursive read access to the complete application folders, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-app-write-recursive`

</td>
<td>

This allows full recursive write access to the complete application folders, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-app-read`

</td>
<td>

This allows non-recursive read access to the application folders.

</td>
</tr>

<tr>
<td>

`fs:allow-app-write`

</td>
<td>

This allows non-recursive write access to the application folders.

</td>
</tr>

<tr>
<td>

`fs:allow-app-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the application folders, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-app-meta`

</td>
<td>

This allows non-recursive read access to metadata of the application folders, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-app-recursive`

</td>
<td>

This scope permits recursive access to the complete application folders, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-app`

</td>
<td>

This scope permits access to all files and list content of top level directories in the application folders.

</td>
</tr>

<tr>
<td>

`fs:scope-app-index`

</td>
<td>

This scope permits to list all files and folders in the application directories.

</td>
</tr>

<tr>
<td>

`fs:allow-appcache-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$APPCACHE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-appcache-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$APPCACHE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-appcache-read`

</td>
<td>

This allows non-recursive read access to the `$APPCACHE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appcache-write`

</td>
<td>

This allows non-recursive write access to the `$APPCACHE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appcache-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$APPCACHE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-appcache-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$APPCACHE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-appcache-recursive`

</td>
<td>

This scope permits recursive access to the complete `$APPCACHE` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-appcache`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$APPCACHE` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-appcache-index`

</td>
<td>

This scope permits to list all files and folders in the `$APPCACHE`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appconfig-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$APPCONFIG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-appconfig-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$APPCONFIG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-appconfig-read`

</td>
<td>

This allows non-recursive read access to the `$APPCONFIG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appconfig-write`

</td>
<td>

This allows non-recursive write access to the `$APPCONFIG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appconfig-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$APPCONFIG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-appconfig-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$APPCONFIG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-appconfig-recursive`

</td>
<td>

This scope permits recursive access to the complete `$APPCONFIG` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-appconfig`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$APPCONFIG` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-appconfig-index`

</td>
<td>

This scope permits to list all files and folders in the `$APPCONFIG`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appdata-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$APPDATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-appdata-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$APPDATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-appdata-read`

</td>
<td>

This allows non-recursive read access to the `$APPDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appdata-write`

</td>
<td>

This allows non-recursive write access to the `$APPDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-appdata-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$APPDATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-appdata-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$APPDATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-appdata-recursive`

</td>
<td>

This scope permits recursive access to the complete `$APPDATA` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-appdata`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$APPDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-appdata-index`

</td>
<td>

This scope permits to list all files and folders in the `$APPDATA`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-applocaldata-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$APPLOCALDATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-applocaldata-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$APPLOCALDATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-applocaldata-read`

</td>
<td>

This allows non-recursive read access to the `$APPLOCALDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-applocaldata-write`

</td>
<td>

This allows non-recursive write access to the `$APPLOCALDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-applocaldata-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$APPLOCALDATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-applocaldata-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$APPLOCALDATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-applocaldata-recursive`

</td>
<td>

This scope permits recursive access to the complete `$APPLOCALDATA` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-applocaldata`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$APPLOCALDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-applocaldata-index`

</td>
<td>

This scope permits to list all files and folders in the `$APPLOCALDATA`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-applog-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$APPLOG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-applog-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$APPLOG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-applog-read`

</td>
<td>

This allows non-recursive read access to the `$APPLOG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-applog-write`

</td>
<td>

This allows non-recursive write access to the `$APPLOG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-applog-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$APPLOG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-applog-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$APPLOG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-applog-recursive`

</td>
<td>

This scope permits recursive access to the complete `$APPLOG` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-applog`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$APPLOG` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-applog-index`

</td>
<td>

This scope permits to list all files and folders in the `$APPLOG`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-audio-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$AUDIO` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-audio-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$AUDIO` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-audio-read`

</td>
<td>

This allows non-recursive read access to the `$AUDIO` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-audio-write`

</td>
<td>

This allows non-recursive write access to the `$AUDIO` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-audio-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$AUDIO` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-audio-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$AUDIO` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-audio-recursive`

</td>
<td>

This scope permits recursive access to the complete `$AUDIO` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-audio`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$AUDIO` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-audio-index`

</td>
<td>

This scope permits to list all files and folders in the `$AUDIO`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-cache-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$CACHE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-cache-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$CACHE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-cache-read`

</td>
<td>

This allows non-recursive read access to the `$CACHE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-cache-write`

</td>
<td>

This allows non-recursive write access to the `$CACHE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-cache-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$CACHE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-cache-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$CACHE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-cache-recursive`

</td>
<td>

This scope permits recursive access to the complete `$CACHE` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-cache`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$CACHE` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-cache-index`

</td>
<td>

This scope permits to list all files and folders in the `$CACHE`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-config-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$CONFIG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-config-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$CONFIG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-config-read`

</td>
<td>

This allows non-recursive read access to the `$CONFIG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-config-write`

</td>
<td>

This allows non-recursive write access to the `$CONFIG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-config-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$CONFIG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-config-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$CONFIG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-config-recursive`

</td>
<td>

This scope permits recursive access to the complete `$CONFIG` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-config`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$CONFIG` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-config-index`

</td>
<td>

This scope permits to list all files and folders in the `$CONFIG`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-data-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$DATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-data-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$DATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-data-read`

</td>
<td>

This allows non-recursive read access to the `$DATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-data-write`

</td>
<td>

This allows non-recursive write access to the `$DATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-data-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$DATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-data-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$DATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-data-recursive`

</td>
<td>

This scope permits recursive access to the complete `$DATA` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-data`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$DATA` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-data-index`

</td>
<td>

This scope permits to list all files and folders in the `$DATA`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-desktop-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$DESKTOP` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-desktop-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$DESKTOP` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-desktop-read`

</td>
<td>

This allows non-recursive read access to the `$DESKTOP` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-desktop-write`

</td>
<td>

This allows non-recursive write access to the `$DESKTOP` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-desktop-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$DESKTOP` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-desktop-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$DESKTOP` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-desktop-recursive`

</td>
<td>

This scope permits recursive access to the complete `$DESKTOP` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-desktop`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$DESKTOP` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-desktop-index`

</td>
<td>

This scope permits to list all files and folders in the `$DESKTOP`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-document-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$DOCUMENT` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-document-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$DOCUMENT` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-document-read`

</td>
<td>

This allows non-recursive read access to the `$DOCUMENT` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-document-write`

</td>
<td>

This allows non-recursive write access to the `$DOCUMENT` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-document-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$DOCUMENT` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-document-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$DOCUMENT` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-document-recursive`

</td>
<td>

This scope permits recursive access to the complete `$DOCUMENT` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-document`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$DOCUMENT` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-document-index`

</td>
<td>

This scope permits to list all files and folders in the `$DOCUMENT`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-download-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$DOWNLOAD` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-download-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$DOWNLOAD` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-download-read`

</td>
<td>

This allows non-recursive read access to the `$DOWNLOAD` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-download-write`

</td>
<td>

This allows non-recursive write access to the `$DOWNLOAD` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-download-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$DOWNLOAD` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-download-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$DOWNLOAD` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-download-recursive`

</td>
<td>

This scope permits recursive access to the complete `$DOWNLOAD` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-download`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$DOWNLOAD` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-download-index`

</td>
<td>

This scope permits to list all files and folders in the `$DOWNLOAD`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-exe-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$EXE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-exe-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$EXE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-exe-read`

</td>
<td>

This allows non-recursive read access to the `$EXE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-exe-write`

</td>
<td>

This allows non-recursive write access to the `$EXE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-exe-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$EXE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-exe-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$EXE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-exe-recursive`

</td>
<td>

This scope permits recursive access to the complete `$EXE` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-exe`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$EXE` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-exe-index`

</td>
<td>

This scope permits to list all files and folders in the `$EXE`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-font-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$FONT` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-font-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$FONT` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-font-read`

</td>
<td>

This allows non-recursive read access to the `$FONT` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-font-write`

</td>
<td>

This allows non-recursive write access to the `$FONT` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-font-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$FONT` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-font-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$FONT` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-font-recursive`

</td>
<td>

This scope permits recursive access to the complete `$FONT` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-font`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$FONT` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-font-index`

</td>
<td>

This scope permits to list all files and folders in the `$FONT`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-home-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$HOME` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-home-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$HOME` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-home-read`

</td>
<td>

This allows non-recursive read access to the `$HOME` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-home-write`

</td>
<td>

This allows non-recursive write access to the `$HOME` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-home-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$HOME` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-home-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$HOME` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-home-recursive`

</td>
<td>

This scope permits recursive access to the complete `$HOME` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-home`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$HOME` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-home-index`

</td>
<td>

This scope permits to list all files and folders in the `$HOME`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-localdata-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$LOCALDATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-localdata-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$LOCALDATA` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-localdata-read`

</td>
<td>

This allows non-recursive read access to the `$LOCALDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-localdata-write`

</td>
<td>

This allows non-recursive write access to the `$LOCALDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-localdata-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$LOCALDATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-localdata-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$LOCALDATA` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-localdata-recursive`

</td>
<td>

This scope permits recursive access to the complete `$LOCALDATA` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-localdata`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$LOCALDATA` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-localdata-index`

</td>
<td>

This scope permits to list all files and folders in the `$LOCALDATA`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-log-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$LOG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-log-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$LOG` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-log-read`

</td>
<td>

This allows non-recursive read access to the `$LOG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-log-write`

</td>
<td>

This allows non-recursive write access to the `$LOG` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-log-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$LOG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-log-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$LOG` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-log-recursive`

</td>
<td>

This scope permits recursive access to the complete `$LOG` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-log`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$LOG` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-log-index`

</td>
<td>

This scope permits to list all files and folders in the `$LOG`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-picture-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$PICTURE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-picture-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$PICTURE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-picture-read`

</td>
<td>

This allows non-recursive read access to the `$PICTURE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-picture-write`

</td>
<td>

This allows non-recursive write access to the `$PICTURE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-picture-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$PICTURE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-picture-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$PICTURE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-picture-recursive`

</td>
<td>

This scope permits recursive access to the complete `$PICTURE` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-picture`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$PICTURE` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-picture-index`

</td>
<td>

This scope permits to list all files and folders in the `$PICTURE`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-public-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$PUBLIC` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-public-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$PUBLIC` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-public-read`

</td>
<td>

This allows non-recursive read access to the `$PUBLIC` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-public-write`

</td>
<td>

This allows non-recursive write access to the `$PUBLIC` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-public-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$PUBLIC` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-public-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$PUBLIC` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-public-recursive`

</td>
<td>

This scope permits recursive access to the complete `$PUBLIC` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-public`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$PUBLIC` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-public-index`

</td>
<td>

This scope permits to list all files and folders in the `$PUBLIC`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-resource-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$RESOURCE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-resource-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$RESOURCE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-resource-read`

</td>
<td>

This allows non-recursive read access to the `$RESOURCE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-resource-write`

</td>
<td>

This allows non-recursive write access to the `$RESOURCE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-resource-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$RESOURCE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-resource-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$RESOURCE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-resource-recursive`

</td>
<td>

This scope permits recursive access to the complete `$RESOURCE` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-resource`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$RESOURCE` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-resource-index`

</td>
<td>

This scope permits to list all files and folders in the `$RESOURCE`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-runtime-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$RUNTIME` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-runtime-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$RUNTIME` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-runtime-read`

</td>
<td>

This allows non-recursive read access to the `$RUNTIME` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-runtime-write`

</td>
<td>

This allows non-recursive write access to the `$RUNTIME` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-runtime-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$RUNTIME` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-runtime-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$RUNTIME` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-runtime-recursive`

</td>
<td>

This scope permits recursive access to the complete `$RUNTIME` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-runtime`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$RUNTIME` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-runtime-index`

</td>
<td>

This scope permits to list all files and folders in the `$RUNTIME`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-temp-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$TEMP` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-temp-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$TEMP` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-temp-read`

</td>
<td>

This allows non-recursive read access to the `$TEMP` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-temp-write`

</td>
<td>

This allows non-recursive write access to the `$TEMP` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-temp-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$TEMP` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-temp-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$TEMP` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-temp-recursive`

</td>
<td>

This scope permits recursive access to the complete `$TEMP` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-temp`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$TEMP` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-temp-index`

</td>
<td>

This scope permits to list all files and folders in the `$TEMP`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-template-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$TEMPLATE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-template-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$TEMPLATE` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-template-read`

</td>
<td>

This allows non-recursive read access to the `$TEMPLATE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-template-write`

</td>
<td>

This allows non-recursive write access to the `$TEMPLATE` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-template-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$TEMPLATE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-template-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$TEMPLATE` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-template-recursive`

</td>
<td>

This scope permits recursive access to the complete `$TEMPLATE` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-template`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$TEMPLATE` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-template-index`

</td>
<td>

This scope permits to list all files and folders in the `$TEMPLATE`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-video-read-recursive`

</td>
<td>

This allows full recursive read access to the complete `$VIDEO` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-video-write-recursive`

</td>
<td>

This allows full recursive write access to the complete `$VIDEO` folder, files and subdirectories.

</td>
</tr>

<tr>
<td>

`fs:allow-video-read`

</td>
<td>

This allows non-recursive read access to the `$VIDEO` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-video-write`

</td>
<td>

This allows non-recursive write access to the `$VIDEO` folder.

</td>
</tr>

<tr>
<td>

`fs:allow-video-meta-recursive`

</td>
<td>

This allows full recursive read access to metadata of the `$VIDEO` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:allow-video-meta`

</td>
<td>

This allows non-recursive read access to metadata of the `$VIDEO` folder, including file listing and statistics.

</td>
</tr>

<tr>
<td>

`fs:scope-video-recursive`

</td>
<td>

This scope permits recursive access to the complete `$VIDEO` folder, including sub directories and files.

</td>
</tr>

<tr>
<td>

`fs:scope-video`

</td>
<td>

This scope permits access to all files and list content of top level directories in the `$VIDEO` folder.

</td>
</tr>

<tr>
<td>

`fs:scope-video-index`

</td>
<td>

This scope permits to list all files and folders in the `$VIDEO`folder.

</td>
</tr>

<tr>
<td>

`fs:allow-copy-file`

</td>
<td>

Enables the copy_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-copy-file`

</td>
<td>

Denies the copy_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-create`

</td>
<td>

Enables the create command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-create`

</td>
<td>

Denies the create command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-exists`

</td>
<td>

Enables the exists command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-exists`

</td>
<td>

Denies the exists command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-fstat`

</td>
<td>

Enables the fstat command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-fstat`

</td>
<td>

Denies the fstat command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-ftruncate`

</td>
<td>

Enables the ftruncate command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-ftruncate`

</td>
<td>

Denies the ftruncate command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-lstat`

</td>
<td>

Enables the lstat command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-lstat`

</td>
<td>

Denies the lstat command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-mkdir`

</td>
<td>

Enables the mkdir command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-mkdir`

</td>
<td>

Denies the mkdir command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-open`

</td>
<td>

Enables the open command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-open`

</td>
<td>

Denies the open command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-read`

</td>
<td>

Enables the read command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-read`

</td>
<td>

Denies the read command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-read-dir`

</td>
<td>

Enables the read_dir command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-read-dir`

</td>
<td>

Denies the read_dir command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-read-file`

</td>
<td>

Enables the read_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-read-file`

</td>
<td>

Denies the read_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-read-text-file`

</td>
<td>

Enables the read_text_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-read-text-file`

</td>
<td>

Denies the read_text_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-read-text-file-lines`

</td>
<td>

Enables the read_text_file_lines command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-read-text-file-lines`

</td>
<td>

Denies the read_text_file_lines command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-read-text-file-lines-next`

</td>
<td>

Enables the read_text_file_lines_next command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-read-text-file-lines-next`

</td>
<td>

Denies the read_text_file_lines_next command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-remove`

</td>
<td>

Enables the remove command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-remove`

</td>
<td>

Denies the remove command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-rename`

</td>
<td>

Enables the rename command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-rename`

</td>
<td>

Denies the rename command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-seek`

</td>
<td>

Enables the seek command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-seek`

</td>
<td>

Denies the seek command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-size`

</td>
<td>

Enables the size command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-size`

</td>
<td>

Denies the size command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-start-accessing-security-scoped-resource`

</td>
<td>

Enables the start_accessing_security_scoped_resource command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-start-accessing-security-scoped-resource`

</td>
<td>

Denies the start_accessing_security_scoped_resource command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-stat`

</td>
<td>

Enables the stat command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-stat`

</td>
<td>

Denies the stat command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-stop-accessing-security-scoped-resource`

</td>
<td>

Enables the stop_accessing_security_scoped_resource command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-stop-accessing-security-scoped-resource`

</td>
<td>

Denies the stop_accessing_security_scoped_resource command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-truncate`

</td>
<td>

Enables the truncate command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-truncate`

</td>
<td>

Denies the truncate command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-unwatch`

</td>
<td>

Enables the unwatch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-unwatch`

</td>
<td>

Denies the unwatch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-watch`

</td>
<td>

Enables the watch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-watch`

</td>
<td>

Denies the watch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-write`

</td>
<td>

Enables the write command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-write`

</td>
<td>

Denies the write command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-write-file`

</td>
<td>

Enables the write_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-write-file`

</td>
<td>

Denies the write_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:allow-write-text-file`

</td>
<td>

Enables the write_text_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:deny-write-text-file`

</td>
<td>

Denies the write_text_file command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`fs:create-app-specific-dirs`

</td>
<td>

This permissions allows to create the application specific directories.


</td>
</tr>

<tr>
<td>

`fs:deny-default`

</td>
<td>

This denies access to dangerous Tauri relevant files and folders by default.

</td>
</tr>

<tr>
<td>

`fs:deny-webview-data-linux`

</td>
<td>

This denies access to the following paths

- `$APPLOCALDATA/CacheStorage/**`
- `$APPLOCALDATA/cookies`
- `$APPLOCALDATA/hsts-storage.sqlite`
- `$APPLOCALDATA/serviceworkers/**`
- `$APPLOCALDATA/WebKitCache/**`
- `$APPLOCALDATA/databases/**`
- `$APPLOCALDATA/localstorage/**`
- `$APPLOCALDATA/mediakeys/**`
- `$APPLOCALDATA/storage/**`

on linux as the webview data and configuration values are stored here.
Allowing access can lead to sensitive information disclosure and should be well considered.

</td>
</tr>

<tr>
<td>

`fs:deny-webview-data-windows`

</td>
<td>

This denies access to the
`$APPLOCALDATA/EBWebView` folder on windows as the webview data and configuration values are stored here.
Allowing access can lead to sensitive information disclosure and should be well considered.

</td>
</tr>

<tr>
<td>

`fs:read-all`

</td>
<td>

This enables all read related commands without any pre-configured accessible paths.

</td>
</tr>

<tr>
<td>

`fs:read-app-specific-dirs-recursive`

</td>
<td>

This permission allows recursive read functionality on the application
specific base directories.


</td>
</tr>

<tr>
<td>

`fs:read-dirs`

</td>
<td>

This enables directory read and file metadata related commands without any pre-configured accessible paths.

</td>
</tr>

<tr>
<td>

`fs:read-files`

</td>
<td>

This enables file read related commands without any pre-configured accessible paths.

</td>
</tr>

<tr>
<td>

`fs:read-meta`

</td>
<td>

This enables all index or metadata related commands without any pre-configured accessible paths.

</td>
</tr>

<tr>
<td>

`fs:scope`

</td>
<td>

An empty permission you can use to modify the global scope.

## Example

```json
{
  "identifier": "read-documents",
  "windows": ["main"],
  "permissions": [
    "fs:allow-read",
    {
      "identifier": "fs:scope",
      "allow": [
        "$APPDATA/documents/**/*"
      ],
      "deny": [
        "$APPDATA/documents/secret.txt"
      ]
    }
  ]
}
```


</td>
</tr>

<tr>
<td>

`fs:write-all`

</td>
<td>

This enables all write related commands without any pre-configured accessible paths.

</td>
</tr>

<tr>
<td>

`fs:write-files`

</td>
<td>

This enables all file write related commands without any pre-configured accessible paths.

</td>
</tr>
</table>
