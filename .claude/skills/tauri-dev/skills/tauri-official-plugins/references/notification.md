# Notifications

> Source: https://v2.tauri.app/plugin/notification/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Send native notifications to the user.

Send native notifications to your user using the notification plugin.

## Supported Platforms

> Platform support table is generated on the site: https://v2.tauri.app/plugin/notification/

## Setup

Install the notifications plugin to get started.

**Automatic:**

        Use your project's package manager to add the dependency:

```sh
# npm
npm run tauri add notification
# yarn
yarn run tauri add notification
# pnpm
pnpm tauri add notification
# bun
bun tauri add notification
# deno
deno task tauri add notification
# cargo
cargo tauri add notification
```

**Manual:**

        1. Run the following command in the `src-tauri` folder to add the plugin to the project's dependencies in `Cargo.toml`:

           ```sh frame=none
           cargo add tauri-plugin-notification
           ```

        2. Modify `lib.rs` to initialize the plugin:

           ```rust title="src-tauri/src/lib.rs" ins={4}
           #[cfg_attr(mobile, tauri::mobile_entry_point)]
           pub fn run() {
               tauri::Builder::default()
                   .plugin(tauri_plugin_notification::init())
                   .run(tauri::generate_context!())
                   .expect("error while running tauri application");
           }
           ```

        3. If you'd like to use notifications in JavaScript then install the npm package as well:

```sh
# npm
npm install @tauri-apps/plugin-notification
# yarn
yarn add @tauri-apps/plugin-notification
# pnpm
pnpm add @tauri-apps/plugin-notification
# deno
deno add npm:@tauri-apps/plugin-notification
# bun
bun add @tauri-apps/plugin-notification
```

## Usage

Here are a few examples of how to use the notification plugin:

- [Send notification to users](#send-notification)
- [Add an action to a notification](#actions)
- [Add an attachment to a notification](#attachments)
- [Send a notification in a specific channel](#channels)

The notification plugin is available in both JavaScript and Rust.

### Send Notification

Follow these steps to send a notification:

1. Check if permission is granted

2. Request permission if not granted

3. Send the notification

**JavaScript:**

```javascript
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification';
// when using `"withGlobalTauri": true`, you may use
// const { isPermissionGranted, requestPermission, sendNotification, } = window.__TAURI__.notification;

// Do you have permission to send a notification?
let permissionGranted = await isPermissionGranted();

// If not we need to request it
if (!permissionGranted) {
  const permission = await requestPermission();
  permissionGranted = permission === 'granted';
}

// Once permission has been granted we can send the notification
if (permissionGranted) {
  sendNotification({ title: 'Tauri', body: 'Tauri is awesome!' });
}
```

**Rust:**

```rust
tauri::Builder::default()
    .plugin(tauri_plugin_notification::init())
    .setup(|app| {
        use tauri_plugin_notification::NotificationExt;
        app.notification()
            .builder()
            .title("Tauri")
            .body("Tauri is awesome")
            .show()
            .unwrap();

        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

### Actions

:::caution[Mobile Only]
The Actions API is only available on mobile platforms.
:::

Actions add interactive buttons and inputs to notifications. Use them to create a responsive experience for your users.

#### Register Action Types

Register action types to define interactive elements:

```javascript
import { registerActionTypes } from '@tauri-apps/plugin-notification';

await registerActionTypes([
  {
    id: 'messages',
    actions: [
      {
        id: 'reply',
        title: 'Reply',
        input: true,
        inputButtonTitle: 'Send',
        inputPlaceholder: 'Type your reply...',
      },
      {
        id: 'mark-read',
        title: 'Mark as Read',
        foreground: false,
      },
    ],
  },
]);
```

#### Action Properties

| Property                 | Description                             |
| ------------------------ | --------------------------------------- |
| `id`                     | Unique identifier for the action        |
| `title`                  | Display text for the action button      |
| `requiresAuthentication` | Requires device authentication          |
| `foreground`             | Brings app to foreground when triggered |
| `destructive`            | Shows action in red on iOS              |
| `input`                  | Enables text input                      |
| `inputButtonTitle`       | Text for input submit button            |
| `inputPlaceholder`       | Placeholder text for input field        |

#### Listen for Actions

Listen to user interactions with notification actions:

```javascript
import { onAction } from '@tauri-apps/plugin-notification';

await onAction((notification) => {
  console.log('Action performed:', notification);
});
```

### Attachments

Attachments add media content to notifications. Support varies by platform.

```javascript
import { sendNotification } from '@tauri-apps/plugin-notification';

sendNotification({
  title: 'New Image',
  body: 'Check out this picture',
  attachments: [
    {
      id: 'image-1',
      url: 'asset:///notification-image.jpg',
    },
  ],
});
```

#### Attachment Properties

| Property | Description                                    |
| -------- | ---------------------------------------------- |
| `id`     | Unique identifier                              |
| `url`    | Content URL using asset:// or file:// protocol |

Note: Test attachments on your target platforms to ensure compatibility.

### Channels

Channels organize notifications into categories with different behaviors. While primarily used on Android, they provide a consistent API across platforms.

#### Create a Channel

```javascript
import {
  createChannel,
  Importance,
  Visibility,
} from '@tauri-apps/plugin-notification';

await createChannel({
  id: 'messages',
  name: 'Messages',
  description: 'Notifications for new messages',
  importance: Importance.High,
  visibility: Visibility.Private,
  lights: true,
  lightColor: '#ff0000',
  vibration: true,
  sound: 'notification_sound',
});
```

#### Channel Properties

| Property      | Description                                    |
| ------------- | ---------------------------------------------- |
| `id`          | Unique identifier                              |
| `name`        | Display name                                   |
| `description` | Purpose description                            |
| `importance`  | Priority level (None, Min, Low, Default, High) |
| `visibility`  | Privacy setting (Secret, Private, Public)      |
| `lights`      | Enable notification LED (Android)              |
| `lightColor`  | LED color (Android)                            |
| `vibration`   | Enable vibrations                              |
| `sound`       | Custom sound filename                          |

#### Managing Channels

List existing channels:

```javascript
import { channels } from '@tauri-apps/plugin-notification';

const existingChannels = await channels();
```

Remove a channel:

```javascript
import { removeChannel } from '@tauri-apps/plugin-notification';

await removeChannel('messages');
```

#### Using Channels

Send a notification using a channel:

```javascript
import { sendNotification } from '@tauri-apps/plugin-notification';

sendNotification({
  title: 'New Message',
  body: 'You have a new message',
  channelId: 'messages',
});
```

Note: Create channels before sending notifications that reference them. Invalid channel IDs prevent notifications from displaying.

## Security Considerations

Aside from normal sanitization procedures of user input there are currently no known security considerations.

See **Permission reference** at the end of this file (generated from the plugin's `permissions/autogenerated/reference.md`).

## Permission reference

> Source: tauri-apps/plugins-workspace `plugins/notification/permissions/autogenerated/reference.md`

## Default Permission

This permission set configures which
notification features are by default exposed.

#### Granted Permissions

It allows all notification related features.

#### This default permission set includes the following:

- `allow-is-permission-granted`
- `allow-request-permission`
- `allow-notify`
- `allow-register-action-types`
- `allow-register-listener`
- `allow-remove-listener`
- `allow-cancel`
- `allow-get-pending`
- `allow-remove-active`
- `allow-get-active`
- `allow-check-permissions`
- `allow-show`
- `allow-batch`
- `allow-list-channels`
- `allow-delete-channel`
- `allow-create-channel`
- `allow-permission-state`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`notification:allow-batch`

</td>
<td>

Enables the batch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-batch`

</td>
<td>

Denies the batch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-cancel`

</td>
<td>

Enables the cancel command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-cancel`

</td>
<td>

Denies the cancel command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-check-permissions`

</td>
<td>

Enables the check_permissions command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-check-permissions`

</td>
<td>

Denies the check_permissions command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-create-channel`

</td>
<td>

Enables the create_channel command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-create-channel`

</td>
<td>

Denies the create_channel command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-delete-channel`

</td>
<td>

Enables the delete_channel command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-delete-channel`

</td>
<td>

Denies the delete_channel command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-get-active`

</td>
<td>

Enables the get_active command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-get-active`

</td>
<td>

Denies the get_active command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-get-pending`

</td>
<td>

Enables the get_pending command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-get-pending`

</td>
<td>

Denies the get_pending command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-is-permission-granted`

</td>
<td>

Enables the is_permission_granted command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-is-permission-granted`

</td>
<td>

Denies the is_permission_granted command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-list-channels`

</td>
<td>

Enables the list_channels command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-list-channels`

</td>
<td>

Denies the list_channels command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-notify`

</td>
<td>

Enables the notify command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-notify`

</td>
<td>

Denies the notify command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-permission-state`

</td>
<td>

Enables the permission_state command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-permission-state`

</td>
<td>

Denies the permission_state command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-register-action-types`

</td>
<td>

Enables the register_action_types command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-register-action-types`

</td>
<td>

Denies the register_action_types command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-register-listener`

</td>
<td>

Enables the register_listener command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-register-listener`

</td>
<td>

Denies the register_listener command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-remove-active`

</td>
<td>

Enables the remove_active command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-remove-active`

</td>
<td>

Denies the remove_active command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-remove-listener`

</td>
<td>

Enables the remove_listener command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-remove-listener`

</td>
<td>

Denies the remove_listener command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-request-permission`

</td>
<td>

Enables the request_permission command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-request-permission`

</td>
<td>

Denies the request_permission command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:allow-show`

</td>
<td>

Enables the show command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`notification:deny-show`

</td>
<td>

Denies the show command without any pre-configured scope.

</td>
</tr>
</table>
