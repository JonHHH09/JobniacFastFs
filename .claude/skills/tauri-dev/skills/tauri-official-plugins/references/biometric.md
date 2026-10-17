# Biometric

> Source: https://v2.tauri.app/plugin/biometric/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Prompt the user for biometric authentication on Android and iOS.

Prompt the user for biometric authentication on Android and iOS.

## Supported Platforms

> Platform support table is generated on the site: https://v2.tauri.app/plugin/biometric/

## Setup

Install the biometric plugin to get started.

**Automatic:**

    Use your project's package manager to add the dependency:

```sh
# npm
npm run tauri add biometric
# yarn
yarn run tauri add biometric
# pnpm
pnpm tauri add biometric
# deno
deno task tauri add biometric
# bun
bun tauri add biometric
# cargo
cargo tauri add biometric
```

**Manual:**

        1.  Run the following command in the `src-tauri` folder to add the plugin to the project's dependencies in `Cargo.toml`:

            ```sh frame=none
            cargo add tauri-plugin-biometric --target 'cfg(any(target_os = "android", target_os = "ios"))'
            ```

        2.  Modify `lib.rs` to initialize the plugin:

            ```rust title="src-tauri/src/lib.rs" ins={5-6}
            #[cfg_attr(mobile, tauri::mobile_entry_point)]
            pub fn run() {
                tauri::Builder::default()
                    .setup(|app| {
                        #[cfg(mobile)]
                        app.handle().plugin(tauri_plugin_biometric::Builder::new().build());
                        Ok(())
                    })
                    .run(tauri::generate_context!())
                    .expect("error while running tauri application");
            }
            ```

        3.  Install the JavaScript Guest bindings using your preferred JavaScript package manager:

```sh
# npm
npm install @tauri-apps/plugin-biometric
# yarn
yarn add @tauri-apps/plugin-biometric
# pnpm
pnpm add @tauri-apps/plugin-biometric
# deno
deno add npm:@tauri-apps/plugin-biometric
# bun
bun add @tauri-apps/plugin-biometric
```

## Configuration

On iOS the biometric plugin requires the `NSFaceIDUsageDescription` information property list value, which should describe why your app needs to use biometric authentication.

In the `src-tauri/Info.ios.plist` file, add the following snippet:

```xml title=src-tauri/Info.ios.plist
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
	<dict>
		<key>NSFaceIDUsageDescription</key>
		<string>Authenticate with biometric</string>
	</dict>
</plist>
```

## Usage

This plugin enables you to verify the availability of Biometric Authentication on a device, prompt the user for biometric authentication, and check the result to determine if the authentication was successful or not.

### Check Status

You can check the status of Biometric Authentication, including its availability and the types of biometric authentication methods supported.

**JavaScript:**

```javascript
import { checkStatus } from '@tauri-apps/plugin-biometric';

const status = await checkStatus();
if (status.isAvailable) {
  console.log('Yes! Biometric Authentication is available');
} else {
  console.log(
    'No! Biometric Authentication is not available due to ' + status.error
  );
}
```

**Rust:**

```rust
use tauri_plugin_biometric::BiometricExt;

fn check_biometric(app_handle: tauri::AppHandle) {
    let status = app_handle.biometric().status().unwrap();
    if status.is_available {
        println!("Yes! Biometric Authentication is available");
    } else {
        println!("No! Biometric Authentication is not available due to: {}", status.error.unwrap());
    }
}
```

### Authenticate

To prompt the user for Biometric Authentication, utilize the `authenticate()` method.

**JavaScript:**

```javascript ins={18}
import { authenticate } from '@tauri-apps/plugin-biometric';

const options = {
  // Set true if you want the user to be able to authenticate using phone password
  allowDeviceCredential: false,
  cancelTitle: "Feature won't work if Canceled",

  // iOS only feature
  fallbackTitle: 'Sorry, authentication failed',

  // Android only features
  title: 'Tauri feature',
  subtitle: 'Authenticate to access the locked Tauri function',
  confirmationRequired: true,
};

try {
  await authenticate('This feature is locked', options);
  console.log(
    'Hooray! Successfully Authenticated! We can now perform the locked Tauri function!'
  );
} catch (err) {
  console.log('Oh no! Authentication failed because ' + err.message);
}
```

**Rust:**

```rust ins={21}
use tauri_plugin_biometric::{BiometricExt, AuthOptions};

fn bio_auth(app_handle: tauri::AppHandle) {

    let options = AuthOptions {
        // Set True if you want the user to be able to authenticate using phone password
        allow_device_credential:false,
        cancel_title: Some("Feature won't work if Canceled".to_string()),

        // iOS only feature
        fallback_title: Some("Sorry, authentication failed".to_string()),

        // Android only features
        title: Some("Tauri feature".to_string()),
        subtitle: Some("Authenticate to access the locked Tauri function".to_string()),
        confirmation_required: Some(true),
    };

    // if the authentication was successful, the function returns Result::Ok()
    // otherwise returns Result::Error()
    match app_handle.biometric().authenticate("This feature is locked".to_string(), options) {
        Ok(_) => {
            println!("Hooray! Successfully Authenticated! We can now perform the locked Tauri function!");
        }
        Err(e) => {
            println!("Oh no! Authentication failed because : {e}");
        }
    }
}
```

## Permissions

By default all potentially dangerous plugin commands and scopes are blocked and cannot be accessed. You must modify the permissions in your `capabilities` configuration to enable these.

See the [Capabilities Overview](https://v2.tauri.app/security/capabilities/) for more information and the [step by step guide](https://v2.tauri.app/learn/security/using-plugin-permissions/) to use plugin permissions.

```json title="src-tauri/capabilities/default.json" ins={6}
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "main-capability",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": ["biometric:default"]
}
```

See **Permission reference** at the end of this file (generated from the plugin's `permissions/autogenerated/reference.md`).

## Permission reference

> Source: tauri-apps/plugins-workspace `plugins/biometric/permissions/autogenerated/reference.md`

## Default Permission

This permission set configures which
biometric features are by default exposed.

#### Granted Permissions

It allows checking the biometric status and
prompting for authentication. Encrypting and
decrypting data with `biometric_cipher` is not
included and must be enabled explicitly with
`biometric:allow-biometric-cipher`.

#### This default permission set includes the following:

- `allow-authenticate`
- `allow-status`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`biometric:allow-authenticate`

</td>
<td>

Enables the authenticate command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`biometric:deny-authenticate`

</td>
<td>

Denies the authenticate command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`biometric:allow-biometric-cipher`

</td>
<td>

Enables the biometric_cipher command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`biometric:deny-biometric-cipher`

</td>
<td>

Denies the biometric_cipher command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`biometric:allow-status`

</td>
<td>

Enables the status command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`biometric:deny-status`

</td>
<td>

Denies the status command without any pre-configured scope.

</td>
</tr>
</table>
