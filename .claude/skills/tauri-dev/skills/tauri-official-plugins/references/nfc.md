# NFC

> Source: https://v2.tauri.app/plugin/nfc/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Read and write NFC tags on Android and iOS.

Read and write NFC tags on Android and iOS.

## Supported Platforms

> Platform support table is generated on the site: https://v2.tauri.app/plugin/nfc/

## Setup

Install the nfc plugin to get started.

**Automatic:**

    Use your project's package manager to add the dependency:

    {' '}

```sh
# npm
npm run tauri add nfc
# yarn
yarn run tauri add nfc
# pnpm
pnpm tauri add nfc
# bun
bun tauri add nfc
# cargo
cargo tauri add nfc
```

**Manual:**

      1.  Run the following command in the `src-tauri` folder to add the plugin to the project's dependencies in `Cargo.toml`:

          ```sh frame=none
          cargo add tauri-plugin-nfc --target 'cfg(any(target_os = "android", target_os = "ios"))'
          ```

      2.  Modify `lib.rs` to initialize the plugin:

          ```rust title="src-tauri/src/lib.rs" ins={5-6}
          #[cfg_attr(mobile, tauri::mobile_entry_point)]
          pub fn run() {
              tauri::Builder::default()
                  .setup(|app| {
                      #[cfg(mobile)]
                      app.handle().plugin(tauri_plugin_nfc::init());
                      Ok(())
                  })
                  .run(tauri::generate_context!())
                  .expect("error while running tauri application");
          }
          ```

      3.  Install the JavaScript Guest bindings using your preferred JavaScript package manager:

```sh
# npm
npm install @tauri-apps/plugin-nfc
# yarn
yarn add @tauri-apps/plugin-nfc
# pnpm
pnpm add @tauri-apps/plugin-nfc
# deno
deno add npm:@tauri-apps/plugin-nfc
# bun
bun add @tauri-apps/plugin-nfc
```

## Configuration

The NFC plugin requires native configuration for iOS.

### iOS

To access the NFC APIs on iOS you must adjust the target iOS version, configure a usage description on the Info.plist file and add the NFC capability to your application.

#### Target IOS version

The NFC plugin requires iOS 14+. This is the default for Tauri applications created with Tauri CLI v2.8 and above, but you can edit your Xcode project to configure it.

In the `src-tauri/gen/apple/<project-name>.xcodeproj/project.pbxproj` file, set all `IPHONEOS_DEPLOYMENT_TARGET` properties to `14.0`:

```pbxproj title="src-tauri/gen/apple/<project-name>.xcodeproj/project.pbxproj"
/* Begin XCBuildConfiguration section */
		1234567890ABCDEF12345678 /* release */ = {
			isa = XCBuildConfiguration;
			buildSettings = {
				/* ... */
				IPHONEOS_DEPLOYMENT_TARGET = 14.0;
			};
			name = release;
		};
		ABCDEF1234567890ABCDEF12 /* debug */ = {
			isa = XCBuildConfiguration;
			buildSettings = {
        /* ... */
				IPHONEOS_DEPLOYMENT_TARGET = 14.0;
			};
			name = debug;
		};
```

Alternatively you can set the deployment target from Xcode in the `General > Minimum Deployments > iOS` configuration.

#### Info.plist

On iOS the NFC plugin requires the `NFCReaderUsageDescription` information property list value, which should describe why your app needs to scan or write to NFC tags.

In the `src-tauri/Info.ios.plist` file, add the following snippet:

```xml title=src-tauri/Info.ios.plist
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
	<dict>
		<key>NFCReaderUsageDescription</key>
		<string>Read and write various NFC tags</string>
	</dict>
</plist>
```

#### NFC Capability

Additionally iOS requires the NFC capability to be associated with your application.

The capability can be added in Xcode in the project configuration's "Signing & Capabilities" tab by clicking the "+ Capability" button and
selecting the "Near Field Communication Tag Reading" capability (see [Add a capability to a target] for more information)
or by adding the following configuration to the `gen/apple/<app-name>_iOS/<app-name>_iOS.entitlements` file:

```xml title="gen/apple/<app-name>_iOS/<app-name>_iOS.entitlements"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.developer.nfc.readersession.formats</key>
	<array>
		<string>TAG</string>
	</array>
</dict>
</plist>
```

## Usage

The NFC plugin is available in both JavaScript and Rust, allowing you to scan and write to NFC tags.

### Checking if NFC is supported

Not every mobile device has the capability to scan NFC tags, so you should check for availability before using the scan and write APIs.

**JavaScript:**

```javascript
import { isAvailable } from '@tauri-apps/plugin-nfc';

const canScanNfc = await isAvailable();
```

**Rust:**

```rust
tauri::Builder::default()
  .setup(|app| {
    #[cfg(mobile)]
    {
      use tauri_plugin_nfc::NfcExt;

      app.handle().plugin(tauri_plugin_nfc::init());

      let can_scan_nfc = app.nfc().is_available()?;
    }
    Ok(())
  })
```

### Scanning NFC tags

The plugin can scan either generic NFC tags or NFC tags with a NDEF (NFC Data Exchange Format) message,
which is a standard format to encapsulate typed data in an NFC tag.

**JavaScript:**

```javascript
import { scan } from '@tauri-apps/plugin-nfc';

const scanType = {
  type: 'ndef', // or 'tag',
};

const options = {
  keepSessionAlive: false,
  // configure the messages displayed in the "Scan NFC" dialog on iOS
  message: 'Scan a NFC tag',
  successMessage: 'NFC tag successfully scanned',
};

const tag = await scan(scanType, options);
```

**Rust:**

```rust
tauri::Builder::default()
  .setup(|app| {
    #[cfg(mobile)]
    {
      use tauri_plugin_nfc::NfcExt;

      app.handle().plugin(tauri_plugin_nfc::init());

      let tag = app
        .nfc()
        .scan(tauri_plugin_nfc::ScanRequest {
            kind: tauri_plugin_nfc::ScanKind::Ndef {
                mime_type: None,
                uri: None,
                tech_list: None,
            },
            keep_session_alive: false,
        })?
        .tag;
    }
    Ok(())
  })
```

:::note
The `keepSessionAlive` option can be used to directly write to the scanned NFC tag later.

If you do not provide that option, the session is recreated on the next `write()` call,
which means the app will try to rescan the tag.
:::

#### Filters

The NFC scanner can also filter tags with a specific URI format, mime type or NFC tag technologies.
In this case, the scan will only detect tags that matches the provided filters.

:::note
Filtering is only available on Android, so you should always check the scanned NFC tag contents.

The mime type is case sensitive and must be provided with lower case letters.
:::

**JavaScript:**

```javascript
import { scan, TechKind } from '@tauri-apps/plugin-nfc';

const techLists = [
  // capture anything using NfcF
  [TechKind.NfcF],
  // capture all MIFARE Classics with NDEF payloads
  [TechKind.NfcA, TechKind.MifareClassic, TechKind.Ndef],
];

const tag = await scan({
  type: 'ndef', // or 'tag'
  mimeType: 'text/plain',
  uri: {
    scheme: 'https',
    host: 'my.domain.com',
    pathPrefix: '/app',
  },
  techLists,
});
```

**Rust:**

```rust
tauri::Builder::default()
  .setup(|app| {
    #[cfg(mobile)]
    {
      use tauri_plugin_nfc::NfcExt;

      app.handle().plugin(tauri_plugin_nfc::init());

      let tag = app
        .nfc()
        .scan(tauri_plugin_nfc::ScanRequest {
            kind: tauri_plugin_nfc::ScanKind::Ndef {
                mime_type: Some("text/plain".to_string()),
                uri: Some(tauri_plugin_nfc::UriFilter {
                  scheme: Some("https".to_string()),
                  host: Some("my.domain.com".to_string()),
                  path_prefix: Some("/app".to_string()),
                }),
                tech_list: Some(vec![
                  vec![tauri_plugin_nfc::TechKind::Ndef],
                ]),
            },
        })?
        .tag;
    }
    Ok(())
  })
```

### Writing to NFC tags

The `write` API can be used to write a payload to a NFC tag.
If there's no scanned tag with `keepSessionAlive: true`, the application will first scan an NFC tag.

**JavaScript:**

```javascript
import { write, textRecord, uriRecord } from '@tauri-apps/plugin-nfc';

const payload = [uriRecord('https://tauri.app'), textRecord('some payload')];

const options = {
  // the kind is only required if you do not have a scanned tag session alive
  // its format is the same as the argument provided to scan()
  kind: {
    type: 'ndef',
  },
  // configure the messages displayed in the "Scan NFC" dialog on iOS
  message: 'Scan a NFC tag',
  successfulReadMessage: 'NFC tag successfully scanned',
  successMessage: 'NFC tag successfully written',
};

await write(payload, options);
```

**Rust:**

:::caution
The Rust API currently only provides a low level interface for writing NFC payloads.

The API will be enhanced soon.
:::

```rust
tauri::Builder::default()
  .setup(|app| {
    #[cfg(mobile)]
    {
      use tauri_plugin_nfc::NfcExt;

      app.handle().plugin(tauri_plugin_nfc::init());

      app
        .nfc()
        .write(vec![
          tauri_plugin_nfc::NfcRecord {
            format: tauri_plugin_nfc::NFCTypeNameFormat::NfcWellKnown,
            kind: vec![0x55], // URI record
            id: vec![],
            payload: vec![], // insert payload here
          }
        ])?;
    }
    Ok(())
  })
```

## Permissions

By default all potentially dangerous plugin commands and scopes are blocked and cannot be accessed. You must modify the permissions in your `capabilities` configuration to enable these.

See the [Capabilities Overview](https://v2.tauri.app/security/capabilities/) for more information and the [step by step guide](https://v2.tauri.app/learn/security/using-plugin-permissions/) to use plugin permissions.

```json title="src-tauri/capabilities/default.json" ins={4}
{
  "permissions": [
    ...,
    "nfc:default",
  ]
}
```

See **Permission reference** at the end of this file (generated from the plugin's `permissions/autogenerated/reference.md`).

[Add a capability to a target]: https://help.apple.com/xcode/mac/current/#/dev88ff319e7

## Permission reference

> Source: tauri-apps/plugins-workspace `plugins/nfc/permissions/autogenerated/reference.md`

## Default Permission

This permission set configures what kind of
operations are available from the nfc plugin.

#### Granted Permissions

Checking if the NFC functionality is available
and scanning nearby tags is allowed.
Writing to tags needs to be manually enabled.

#### This default permission set includes the following:

- `allow-is-available`
- `allow-scan`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`nfc:allow-is-available`

</td>
<td>

Enables the is_available command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`nfc:deny-is-available`

</td>
<td>

Denies the is_available command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`nfc:allow-scan`

</td>
<td>

Enables the scan command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`nfc:deny-scan`

</td>
<td>

Denies the scan command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`nfc:allow-write`

</td>
<td>

Enables the write command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`nfc:deny-write`

</td>
<td>

Denies the write command without any pre-configured scope.

</td>
</tr>
</table>
