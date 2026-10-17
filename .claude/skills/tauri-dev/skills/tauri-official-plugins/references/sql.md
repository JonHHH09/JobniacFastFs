# SQL

> Source: https://v2.tauri.app/plugin/sql/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Tauri Plugin providing an interface for the frontend to communicate with SQL databases through sqlx.

Plugin providing an interface for the frontend to communicate with SQL databases through [sqlx](https://github.com/launchbadge/sqlx). It supports the SQLite, MySQL and PostgreSQL drivers, enabled by a Cargo feature.

## Supported Platforms

> Platform support table is generated on the site: https://v2.tauri.app/plugin/sql/

## Setup

Install the SQL plugin to get started.

**Automatic:**

        Use your project's package manager to add the dependency:

```sh
# npm
npm run tauri add sql
# yarn
yarn run tauri add sql
# pnpm
pnpm tauri add sql
# bun
bun tauri add sql
# deno
deno task tauri add sql
# cargo
cargo tauri add sql
```

**Manual:**

        1.  Run the following command in the `src-tauri` folder to add the plugin to the project's dependencies in `Cargo.toml`:

            ```sh frame=none
            cargo add tauri-plugin-sql
            ```

        2.  Modify `lib.rs` to initialize the plugin:

            ```rust title="src-tauri/src/lib.rs" ins={4}
            #[cfg_attr(mobile, tauri::mobile_entry_point)]
    		    pub fn run() {
                tauri::Builder::default()
                    .plugin(tauri_plugin_sql::Builder::default().build())
                    .run(tauri::generate_context!())
                    .expect("error while running tauri application");
            }
            ```

        3.	Install the JavaScript Guest bindings using your preferred JavaScript package manager:

```sh
# npm
npm install @tauri-apps/plugin-sql
# yarn
yarn add @tauri-apps/plugin-sql
# pnpm
pnpm add @tauri-apps/plugin-sql
# deno
deno add npm:@tauri-apps/plugin-sql
# bun
bun add @tauri-apps/plugin-sql
```

After installing the plugin, you must select the supported database engine.
The available engines are Sqlite, MySQL and PostgreSQL.
Run the following command in the `src-tauri` folder to enable your preferred engine:

**SQLite:**

    ```sh frame=none
    cargo add tauri-plugin-sql --features sqlite
    ```

**MySQL:**

    ```sh frame=none
    cargo add tauri-plugin-sql --features mysql
    ```

**PostgreSQL:**

    ```sh frame=none
    cargo add tauri-plugin-sql --features postgres
    ```

## Usage

All the plugin's APIs are available through the JavaScript guest bindings:

**SQLite:**

The path is relative to [`tauri::api::path::BaseDirectory::AppConfig`](https://docs.rs/tauri/2.0.0/tauri/path/enum.BaseDirectory.html#variant.AppConfig).

```javascript
import Database from '@tauri-apps/plugin-sql';
// when using `"withGlobalTauri": true`, you may use
// const Database = window.__TAURI__.sql;

const db = await Database.load('sqlite:test.db');
await db.execute('INSERT INTO ...');
```

**MySQL:**

```javascript
import Database from '@tauri-apps/plugin-sql';
// when using `"withGlobalTauri": true`, you may use
// const Database = window.__TAURI__.sql;

const db = await Database.load('mysql://user:password@host/test');
await db.execute('INSERT INTO ...');
```

**PostgreSQL:**

```javascript
import Database from '@tauri-apps/plugin-sql';
// when using `"withGlobalTauri": true`, you may use
// const Database = window.__TAURI__.sql;

const db = await Database.load('postgres://user:password@host/test');
await db.execute('INSERT INTO ...');
```

## Syntax

We use [sqlx](https://docs.rs/sqlx/latest/sqlx/) as the underlying library and adopt their query syntax.

**SQLite:**

Use the "$#" syntax when substituting query data

```javascript
const result = await db.execute(
  'INSERT into todos (id, title, status) VALUES ($1, $2, $3)',
  [todos.id, todos.title, todos.status]
);

const result = await db.execute(
  'UPDATE todos SET title = $1, status = $2 WHERE id = $3',
  [todos.title, todos.status, todos.id]
);
```

**MySQL:**

Use "?" when substituting query data

```javascript
const result = await db.execute(
  'INSERT into todos (id, title, status) VALUES (?, ?, ?)',
  [todos.id, todos.title, todos.status]
);

const result = await db.execute(
  'UPDATE todos SET title = ?, status = ? WHERE id = ?',
  [todos.title, todos.status, todos.id]
);
```

**PostgreSQL:**

Use the "$#" syntax when substituting query data

```javascript
const result = await db.execute(
  'INSERT into todos (id, title, status) VALUES ($1, $2, $3)',
  [todos.id, todos.title, todos.status]
);

const result = await db.execute(
  'UPDATE todos SET title = $1, status = $2 WHERE id = $3',
  [todos.title, todos.status, todos.id]
);
```

## Migrations

This plugin supports database migrations, allowing you to manage database schema evolution over time.

### Defining Migrations

Migrations are defined in Rust using the [`Migration`](https://docs.rs/tauri-plugin-sql/latest/tauri_plugin_sql/struct.Migration.html) struct. Each migration should include a unique version number, a description, the SQL to be executed, and the type of migration (Up or Down).

Example of a migration:

```rust
use tauri_plugin_sql::{Migration, MigrationKind};

let migration = Migration {
    version: 1,
    description: "create_initial_tables",
    sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
    kind: MigrationKind::Up,
};
```

Or if you want to use SQL from a file, you can include it by using `include_str!`:

```rust
use tauri_plugin_sql::{Migration, MigrationKind};

let migration = Migration {
    version: 1,
    description: "create_initial_tables",
    sql: include_str!("../drizzle/0000_graceful_boomer.sql"),
    kind: MigrationKind::Up,
};
```

### Adding Migrations to the Plugin Builder

Migrations are registered with the [`Builder`](https://docs.rs/tauri-plugin-sql/latest/tauri_plugin_sql/struct.Builder.html) struct provided by the plugin. Use the `add_migrations` method to add your migrations to the plugin for a specific database connection.

Example of adding migrations:

```rust title="src-tauri/src/main.rs" {1} {6-11} {17}
use tauri_plugin_sql::{Builder, Migration, MigrationKind};

fn main() {
    let migrations = vec![
        // Define your migrations here
        Migration {
            version: 1,
            description: "create_initial_tables",
            sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
            kind: MigrationKind::Up,
        }
    ];

    tauri::Builder::default()
        .plugin(
            tauri_plugin_sql::Builder::default()
                .add_migrations("sqlite:mydatabase.db", migrations)
                .build(),
        )
        ...
}
```

### Applying Migrations

To apply the migrations when the plugin is initialized, add the connection string to the `tauri.conf.json` file:

```json title="src-tauri/tauri.conf.json" {3-5}
{
  "plugins": {
    "sql": {
      "preload": ["sqlite:mydatabase.db"]
    }
  }
}
```

Alternatively, the client side `load()` also runs the migrations for a given connection string:

```ts
import Database from '@tauri-apps/plugin-sql';
const db = await Database.load('sqlite:mydatabase.db');
```

Ensure that the migrations are defined in the correct order and are safe to run multiple times.

:::note
All migrations are executed within a transaction, ensuring atomicity. If any migration fails, the entire transaction is rolled back, leaving the database in a consistent state.
:::

### Migration Management

- **Version Control**: Each migration must have a unique version number. This is crucial for ensuring the migrations are applied in the correct order.
- **Idempotency**: Write migrations in a way that they can be safely re-run without causing errors or unintended consequences.
- **Testing**: Thoroughly test migrations to ensure they work as expected and do not compromise the integrity of your database.

## Permissions

By default all potentially dangerous plugin commands and scopes are blocked and cannot be accessed. You must modify the permissions in your `capabilities` configuration to enable these.

See the [Capabilities Overview](https://v2.tauri.app/security/capabilities/) for more information and the [step by step guide](https://v2.tauri.app/learn/security/using-plugin-permissions/) to use plugin permissions.

```json title="src-tauri/capabilities/default.json" ins={4-5}
{
  "permissions": [
    ...,
    "sql:default",
    "sql:allow-execute",
  ]
}
```

See **Permission reference** at the end of this file (generated from the plugin's `permissions/autogenerated/reference.md`).

## Permission reference

> Source: tauri-apps/plugins-workspace `plugins/sql/permissions/autogenerated/reference.md`

## Default Permission

### Default Permissions

This permission set configures what kind of
database operations are available from the sql plugin.

### Granted Permissions

All reading related operations are enabled.
Also allows to load or close a connection.

#### This default permission set includes the following:

- `allow-close`
- `allow-load`
- `allow-select`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`sql:allow-close`

</td>
<td>

Enables the close command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:deny-close`

</td>
<td>

Denies the close command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:allow-execute`

</td>
<td>

Enables the execute command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:deny-execute`

</td>
<td>

Denies the execute command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:allow-load`

</td>
<td>

Enables the load command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:deny-load`

</td>
<td>

Denies the load command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:allow-select`

</td>
<td>

Enables the select command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`sql:deny-select`

</td>
<td>

Denies the select command without any pre-configured scope.

</td>
</tr>
</table>
