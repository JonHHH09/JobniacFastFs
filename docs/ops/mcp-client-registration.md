# MCP client registration

How to point an MCP client (Claude Desktop, Claude Code or another client that reads an
`mcpServers` block) at JobniacFast. Two transports exist:

- **STDIO** (`jobniac-cli mcp`): the client starts the process and talks over stdin and stdout. This is
  the normal choice. The process opens no network listener.
- **Loopback HTTP** (`jobniac-cli http`): opt-in. The process listens on `127.0.0.1` only and needs a
  bearer token.

Replace `<you>` and other values in `<angle brackets>`. The JSON shapes are the ones the clients read;
JSON needs each backslash in a Windows path doubled.

## 0. Find the binary

The CLI is `jobniac-cli.exe`, installed next to `jobniac.exe`. With the per-user installer the folder
is expected to be `%LOCALAPPDATA%\JobniacFast` (confirm with the step in
[clean-machine smoke, step 7](clean-machine-smoke-checklist.md#3-install)).

```powershell
(Resolve-Path "$env:LOCALAPPDATA\JobniacFast\jobniac-cli.exe").Path
```

Expected: one absolute path ending in `jobniac-cli.exe`. Use that path as `command`.

## 1. Database location

The CLI reads its configuration from the environment only: no `.env` file, no secrets in arguments.
The MCP client passes the `env` block to the process.

The data is one SQLite file. Nothing needs to be installed or set up, and no secret is involved: the
CLI creates the file and brings its schema up to date when it starts.

| Variable | Required | Value |
|---|---|---|
| `JOBNIAC_DB_PATH` | no | absolute path of the database file; without it the per-user default is used |

The default is `%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3` on Windows and
`$XDG_DATA_HOME/jobniac/data/jobniac.sqlite3` (or `~/.local/share/jobniac/data/jobniac.sqlite3`) on
Linux. The desktop app uses the same default, so it and the MCP server share one file; set the same
`JOBNIAC_DB_PATH` for both to share another file.

If the file cannot be opened (for example `JOBNIAC_DB_PATH` is not an absolute path, or the file was
written by a newer version), the server still starts, but every database call fails and `health`
reports `DOWN`.

## 2. Optional opt-in flags

Each opt-in flag is on only when its value is exactly `1`. `true`, `yes`, `01` and values with spaces
are off.

| Variable | Adds | Also needs |
|---|---|---|
| `JOBNIAC_LINK_FETCH_ENABLED=1` | `add_job_from_link` (35 tools) | Windows or Linux; database |
| `JOBNIAC_ARBEITNOW_ENABLED=1` | `scan_arbeitnow_jobs`, `import_arbeitnow_job` (36) | `JOBNIAC_ARBEITNOW_CANDIDATE_KEY` and a valid `JOBNIAC_CURSOR_HMAC_SECRET`; Windows or Linux |
| `JOBNIAC_DELETION_ENABLED=1` | `delete_profile`, `delete_job` (36) | database |
| `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED=1` | `get_document_artifact` (35) | database |
| `JOBNIAC_GENERATED_ENABLED=1` | no new tools; generation starts working | `setup-generated-root` run once; database |
| `JOBNIAC_DOCUMENT_IMPORT_ROOT` | (no flag) import of PDFs from one absolute folder | an absolute, existing folder |
| `JOBNIAC_CURSOR_HMAC_SECRET` | (no flag) keeps cursors valid across restarts | 32 or more characters; unset gives a random key per process |

The default catalog without flags is 34 tools; all flags together give 40.
Turn on only what you use. Deletion and link fetching change data or reach the network.

## 3. Client configuration

### Claude Desktop

File: `%APPDATA%\Claude\claude_desktop_config.json`. Add the `jobniac` entry under `mcpServers`
(create the file with this content if it does not exist). Restart the client after saving.

```json
{
  "mcpServers": {
    "jobniac": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\JobniacFast\\jobniac-cli.exe",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

### Claude Code

Use a project file `.mcp.json` in the project folder (or the user-level equivalent your Claude Code
version uses). The shape is the same; the server name is `jobniac`.

```json
{
  "mcpServers": {
    "jobniac": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\JobniacFast\\jobniac-cli.exe",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

Then check with `claude mcp list`; `jobniac` should show as connected. (Add `"JOBNIAC_DB_PATH": "<absolute
path>"` to `env` only to use a database file other than the default.)

Other clients: use the same `command`, `args` and `env`. A client that asks for a `cwd` may use the
install folder; the CLI does not need one.

## 4. Verify the STDIO mode

First check the process runs and that the CLI is the right build:

```powershell
& "C:\Users\<you>\AppData\Local\JobniacFast\jobniac-cli.exe" --version
```

Expected: `Jobniac <version>`, exit code 0.

Then drive the server by hand. Create the file `mcp-check.jsonl` (in a folder outside the repository)
with exactly these four lines, using Write or an editor:

```text
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owner-check","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health","arguments":{}}}
```

To use a database file other than the default, set `$env:JOBNIAC_DB_PATH` to its absolute path in the same
PowerShell session. Then run:

```powershell
Get-Content .\mcp-check.jsonl | & "C:\Users\<you>\AppData\Local\JobniacFast\jobniac-cli.exe" mcp | Out-File -FilePath .\mcp-out.jsonl -Encoding utf8
Get-Content .\mcp-out.jsonl | ForEach-Object { $_ | ConvertFrom-Json | Out-Null }
(Get-Content .\mcp-out.jsonl | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object id -eq 2).result.tools.Count
```

Expected:

- Three response lines (ids 1, 2 and 3). The second loop completes with no error, which means every
  line of stdout is JSON.
- The last command prints `34` with no opt-in flags set.
- The id 3 response has no `error` and its `result.isError` is not `true`. Its content reports `status`
  `UP` when the database file opens.

Failures:

- A non-JSON line, or any text on stdout: a defect (stdout must carry JSON-RPC only). Record the line.
- No output and an error on stderr: the process did not start; check `--version` first.
- `status` `DOWN`: check the section 1 location (`JOBNIAC_DB_PATH`) and that the file can be created.
- A count other than `34`: an opt-in flag is set in your session, or the build is different.

Protocol version: the server negotiates up to `2025-11-25`, so `2025-06-18` is supported. Which version the snapshot pins is an open owner question
([release owner checklist](release-owner-checklist.md#open-questions-for-the-owner)).

## 5. Loopback HTTP mode (opt-in)

The HTTP mode serves Streamable HTTP at `http://127.0.0.1:<port>/mcp`. It listens on `127.0.0.1`
only. Every request needs `Authorization: Bearer <token>`. Opt-in tools are available to every
authenticated session, so enable only the flags you need.

### 5.1 Generate a token

Generate it in the PowerShell session that starts the server, and do not write it to a file:

```powershell
$bytes = New-Object byte[] 48
$rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
$rng.GetBytes($bytes)
$rng.Dispose()
$token = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
$token.Length
```

Expected: `64`. The token uses only `A-Z a-z 0-9 - _`, which the server requires (43 to 128 characters).

### 5.2 Start the server

Use a port from 1024 to 65535 that nothing else uses. Keep the token in the environment, never in an
argument list:

```powershell
$env:JOBNIAC_HTTP_BEARER_TOKEN = $token
$env:JOBNIAC_HTTP_PORT = '<port>'
& "C:\Users\<you>\AppData\Local\JobniacFast\jobniac-cli.exe" http
```

Expected: stderr shows `Jobniac MCP HTTP starting on 127.0.0.1:<port>/mcp`, and the process keeps
running. Stop it with Ctrl+C (exit code 130).

Failures:

- Exit code 1 with `Jobniac could not start. Check local configuration.`: the port or token is invalid
  (port below 1024 or not digits, token shorter than 43 characters or with other characters).
- Exit code 3: the port cannot be bound (another process uses it).

When the run is over, clear the session values:
`Remove-Item Env:JOBNIAC_HTTP_BEARER_TOKEN` and `Remove-Item Env:JOBNIAC_HTTP_PORT`.

### 5.3 Verify from a second window

The `Host` and `Origin` checks compare against `127.0.0.1:<port>`, so use `127.0.0.1`, not `localhost`.

```powershell
curl.exe -s -i -X POST "http://127.0.0.1:<port>/mcp" -H "Authorization: Bearer $token" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owner-check","version":"0"}}}'
curl.exe -s -i -X POST "http://127.0.0.1:<port>/mcp" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owner-check","version":"0"}}}'
```

Expected: the first request gets an `initialize` result. The second, without the header, is refused
(the response is an error status, not a result). Any other outcome is a failure.

### 5.4 Client configuration

Clients that accept a URL and headers take the URL `http://127.0.0.1:<port>/mcp` and the header
`Authorization: Bearer <token>`. The client stores the header in its own configuration, so that file
then holds the token. Keep it out of shell history and out of command lines you paste.

## 6. Limits of this check

The checks above prove that the process answers correctly and that its catalog has the expected size.
They do not prove a client's own behaviour. Use the client's tool list as the final check.

## 7. Remove a registration

To remove JobniacFast, delete the `jobniac` entry from the client file, and restart the client.
