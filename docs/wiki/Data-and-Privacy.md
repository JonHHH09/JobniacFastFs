# Data and Privacy

JobniacFast is local-first. Your profiles, jobs, match reports and generated PDFs stay on your machine.

## Where data is stored

Your data is one SQLite file. The app and `jobniac-cli` create it and bring it up to date the first time they start. The default location is per user and outside the install folder, so an update or a reinstall never touches it:

| System | Default file |
|---|---|
| Windows | `%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3` |
| macOS | `<home>/Library/Application Support/Jobniac/data/jobniac.sqlite3` |
| Linux | `$XDG_DATA_HOME/jobniac/data/jobniac.sqlite3`, or `<home>/.local/share/jobniac/data/jobniac.sqlite3` when `XDG_DATA_HOME` is not set |

Set `JOBNIAC_DB_PATH` to an absolute file path to use another file. A value that is not a clean absolute path stops the database from opening. It is never replaced by the default.

The desktop app, the built-in MCP server and `jobniac-cli` use the same file and can run at the same time.

Generated PDFs are written to a separate folder, by default `%LOCALAPPDATA%\Jobniac\generated` on Windows, `<home>/Library/Application Support/Jobniac/generated` on macOS and `$XDG_DATA_HOME/jobniac/generated` on Linux. Set `JOBNIAC_GENERATED_ROOT` to choose another folder.

### Back up and restore

Close the app and any running `jobniac-cli`, then copy `jobniac.sqlite3`. If a `jobniac.sqlite3-wal` or `jobniac.sqlite3-shm` file is next to it, copy that too. To restore, copy the file back while the app is closed. A file written by a newer version of the app is refused by an older one.

### Remove your data

Uninstalling the app keeps your data. To remove it, delete the database file and the generated folder after you uninstall.

## What never leaves your machine

- Profiles, jobs, match reports, reviews and stored PDFs are kept in the local database. The app has no account and no cloud storage.
- The MCP server listens on this computer only. The stdio mode opens no listener, and the HTTP mode binds to `127.0.0.1`.
- File paths, tokens and secrets are not sent to the user interface.
- Configuration comes from environment variables only. There is no `.env` file, and secrets are never read from command-line arguments.

The only network traffic the app makes on its own is the update check (see [Updates](Updates.md)), which asks GitHub for the latest release. It runs only when you choose **Check for updates**, or after startup if you turn that on. The setting is off by default.

## Opt-in features

These features are off unless their variable is set to exactly `1`. `true`, `yes` and `01` are off. Set a variable in the environment that starts the app or the MCP client.

| Variable | Feature |
|---|---|
| `JOBNIAC_LINK_FETCH_ENABLED` | Add a job from a web link. Fetches the page. |
| `JOBNIAC_ARBEITNOW_ENABLED` | Scan and import jobs from the Arbeitnow board. Also needs `JOBNIAC_ARBEITNOW_CANDIDATE_KEY` and a valid `JOBNIAC_CURSOR_HMAC_SECRET` (32 characters or more). |
| `JOBNIAC_DELETION_ENABLED` | Delete profiles and jobs. |
| `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED` | Document artifacts and PDF export. |
| `JOBNIAC_GENERATED_ENABLED` | PDF generation and cleanup. Run `jobniac-cli setup-generated-root` once first. |
| `JOBNIAC_LOG_FILE` | Write a `jobniac.log` file in the application log folder. |

The HTTP listener is also opt-in. `jobniac-cli http` starts only when you set `JOBNIAC_HTTP_PORT` (1024 to 65535) and `JOBNIAC_HTTP_BEARER_TOKEN` (43 to 128 characters from `A-Z a-z 0-9 - _`), and every request needs the token. The desktop app can start the same loopback server from Settings, with a new access token each time it starts. See [MCP Server](MCP-Server.md).

Importing PDFs also needs `JOBNIAC_DOCUMENT_IMPORT_ROOT`, an absolute folder that PDFs are read from. Nothing outside it is read.

Turn on only what you use. Deletion and link fetching change data or reach the network.

## Worker sandboxing

PDF reading and rendering, link fetch and the Arbeitnow board run in short-lived worker child processes. Each worker is confined before it handles untrusted input, and the PDF workers get no network access.

- Windows: each worker runs in its own Job Object with limits on memory, CPU time and process count, UI restrictions, and kill-on-close. The limits are set before the worker is allowed to run.
- Linux: the worker sets `no_new_privs`, applies a Landlock ruleset that grants no file access, and a seccomp filter that blocks calls such as `execve`, `ptrace`, mounts and (for PDF workers) all sockets. A kernel without Landlock runs with what it offers.
- macOS: each worker runs in its own process group with resource limits and a Seatbelt sandbox profile: no file writes, no process creation, and no network for the PDF workers. The fetch workers may make outbound TCP and DNS connections only.

## Related

- [Installation](Installation.md)
- [Troubleshooting](Troubleshooting.md)
