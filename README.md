# JobniacFast

JobniacFast is a local-first desktop app and MCP server for managing candidate profiles and job
postings, matching them deterministically, and generating tailored resume and cover-letter PDFs
(generic, Canadian English and French, and German). It is built from a Rust core and a React and
TypeScript user interface on Tauri 2, and stores its data in an embedded SQLite file. Your data
stays on your machine.

## Status

No release has been published yet. The release is being prepared: the first release will be a draft
on GitHub Releases that the owner publishes by hand.

## Requirements

- Windows 11 (x64), Linux (x64, AppImage or `.deb`) or macOS 14 or later on Apple Silicon (`.dmg`).
  Intel Macs are not supported.
- WebView2 on Windows. Windows 11 includes it; the installer installs it if it is missing.

## Install

Releases are published on the [GitHub Releases page](https://github.com/JonHHH09/JobniacFast/releases).

**Windows:**

1. Download `JobniacFast_<version>_x64-setup.exe` and `SHA256SUMS`.
2. Check the installer before you run it:
   ```powershell
   Get-FileHash -Algorithm SHA256 .\JobniacFast_<version>_x64-setup.exe
   Select-String -Path .\SHA256SUMS -Pattern 'x64-setup'
   ```
   The two hashes must be the same. If they differ, do not run the installer.
3. Run the installer. It installs for your user only and does not ask for administrator rights.
   It installs to `%LOCALAPPDATA%\JobniacFast`.

**Expect a SmartScreen warning.** Until the Windows build is signed, Windows shows "Windows protected
your PC". Click **More info**, then **Run anyway**, after you have checked the hash. The warning is
expected for unsigned downloads.

**Linux:** download the AppImage or the `.deb`, and check it against `SHA256SUMS` in the same way
(`sha256sum -c SHA256SUMS --ignore-missing`).

**macOS (Apple Silicon):** download `JobniacFast_<version>_aarch64.dmg` and `SHA256SUMS`, check the
download (`shasum -a 256 -c SHA256SUMS --ignore-missing`), open the DMG and drag JobniacFast to
Applications. A release signed with a Developer ID certificate and notarized by Apple opens normally. A
release without that signature is only ad-hoc signed: macOS then says it cannot verify the app. After you
have checked the hash, open System Settings, Privacy & Security, and choose **Open Anyway** for
JobniacFast (or run `xattr -dr com.apple.quarantine /Applications/JobniacFast.app`). The release notes
say which kind of signature the release has.

**Uninstall:** on Windows use Settings, Apps, JobniacFast, Uninstall, or the `uninstall.exe` in the install
folder. On macOS drag JobniacFast from Applications to the Bin; your data stays (see below).

## Data storage

Your data is one SQLite file. The app and `jobniac-cli` create it, and bring it up to date, the
first time they start; there is nothing to install or configure. The default location is per user
and outside the install folder, so an update or a reinstall never touches it:

- Windows: `%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3`
- macOS: `~/Library/Application Support/Jobniac/data/jobniac.sqlite3`
- Linux: `$XDG_DATA_HOME/jobniac/data/jobniac.sqlite3`, or `~/.local/share/jobniac/data/jobniac.sqlite3`
  when `XDG_DATA_HOME` is not set

Set `JOBNIAC_DB_PATH` to an absolute file path to use another file. A value that is not a clean
absolute path stops the database from opening; it is never replaced by the default.

The desktop app and `jobniac-cli` can use the same file at the same time. To back up, close the app
and any running `jobniac-cli`, then copy `jobniac.sqlite3` (and a `jobniac.sqlite3-wal` or
`jobniac.sqlite3-shm` file next to it, if one is left). To restore, copy the file back while the app
is closed.

`jobniac-cli migrate` applies the schema to the file explicitly:

```powershell
& "$env:LOCALAPPDATA\JobniacFast\jobniac-cli.exe" migrate
```

A new file prints `Applied migrations: ...`; a file that is already current prints
`Schema is current (...); nothing to apply.` A file written by a newer version of the app is refused.

## Configuration

All configuration comes from environment variables. There is no `.env` file, and secrets are never
read from command-line arguments. An option marked **exact `1`** is on only when its value is exactly
`1`; `true`, `yes` and `01` are off.

| Variable | Default | Effect |
|---|---|---|
| `JOBNIAC_DB_PATH` | per-user default (see Data storage) | Absolute path of the SQLite database file. If the file cannot be opened, database features report `DOWN`. |
| `JOBNIAC_DOCUMENT_IMPORT_ROOT` | none | Absolute folder that PDFs can be imported from. |
| `JOBNIAC_GENERATED_ROOT` | `%LOCALAPPDATA%\Jobniac\generated` | Folder for generated PDFs. Set it only to choose another folder. |
| `JOBNIAC_GENERATED_ENABLED` | off (**exact `1`**) | Allows PDF generation and cleanup. Run `jobniac-cli setup-generated-root` once first. |
| `JOBNIAC_LINK_FETCH_ENABLED` | off (**exact `1`**) | Allows adding a job from a web link (fetches the page). |
| `JOBNIAC_ARBEITNOW_ENABLED` | off (**exact `1`**) | Allows scanning and importing Arbeitnow jobs. Also needs `JOBNIAC_ARBEITNOW_CANDIDATE_KEY` and a cursor secret. |
| `JOBNIAC_ARBEITNOW_CANDIDATE_KEY` | none | Signing key for Arbeitnow candidate tokens (required when the flag is `1`). |
| `JOBNIAC_DELETION_ENABLED` | off (**exact `1`**) | Allows deleting profiles and jobs. |
| `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED` | off (**exact `1`**) | Allows document artifacts and export. |
| `JOBNIAC_CURSOR_HMAC_SECRET` | random per process | Keeps paging cursors valid across restarts. 32 characters or more. |
| `JOBNIAC_HTTP_PORT` | none | Port for the loopback HTTP MCP server (1024 to 65535). Used with `jobniac-cli http` only. |
| `JOBNIAC_HTTP_BEARER_TOKEN` | none | Bearer token for the HTTP server: 43 to 128 characters, `A-Z a-z 0-9 - _`. |

Opt-in features are off by default. Deletion and link fetching change data or reach the network, so
turn on only what you use.

## MCP client registration

JobniacFast includes an MCP server. The stdio mode is started by the MCP client:

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

The full guide covers Claude Desktop and Claude Code, the loopback HTTP mode with its token, and how to
check the registration: [docs/ops/mcp-client-registration.md](docs/ops/mcp-client-registration.md).
The default tool list has 34 tools; the opt-in flags add tools (see that guide).

## Command-line tool

`jobniac-cli` has these modes (exit code 0 for success, 1 for a failure with a fixed message, 2 for a usage error):

| Command | Purpose |
|---|---|
| `mcp` | MCP server over stdio. Writes JSON-RPC only to stdout. |
| `http` | Loopback Streamable HTTP MCP server at `http://127.0.0.1:<port>/mcp`. Needs the port and token variables. |
| `migrate` | Applies pending schema migrations. The only command that changes the schema. |
| `cleanup` | One bounded pass over generated files. Prints a JSON summary. |
| `setup-generated-root` | Creates and registers the generated-files folder. Does not enable generation. |
| `--version`, `help` | Version and usage. |

## Development

Requirements: Rust (stable, or the version in `rust-toolchain.toml`), Node.js 22.12 or later, pnpm, and
the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform.

The PDFium library is not committed. Fetch the pinned, checksum-verified build into `vendor/pdfium/`
once before the first build (`scripts/fetch-pdfium.ps1 -Platform windows-x64` on Windows); without it
the desktop build fails with ``resource path `../vendor/pdfium/...` doesn't exist``.

```text
scripts/fetch-pdfium.sh linux-x64   # fetch the pinned PDFium library (Linux)
scripts/fetch-pdfium.sh mac-arm64   # fetch the pinned PDFium library (macOS, Apple Silicon; mac-x64 on Intel)
pnpm install --frozen-lockfile      # install the locked UI dependencies
pnpm tauri dev                      # run the desktop app
pnpm build                          # type-check and build the UI
pnpm tauri build                    # build a release bundle
```

On macOS the bundle config `src-tauri/tauri.macos.conf.json` names the Apple Silicon library
(`mac-arm64`) only, and bundles it into `Contents/Frameworks` of the app, where the bundler signs it with
the app (code in `Contents/Resources` is not signed as code). The default build is ad-hoc signed, with the
entitlement in `src-tauri/macos/Entitlements.adhoc.plist` so that the hardened runtime loads that library;
the release build uses a Developer ID when the repository secrets exist (see
`docs/ops/release-owner-checklist.md`). Intel Macs are not supported: `mac-x64` is only a fetch target.

Rust checks, from the repository root (the Cargo workspace is at the root). The PDF tests load PDFium
from `JOBNIAC_PDFIUM_DIR`, so set it to the absolute path of `vendor/pdfium/<platform>` first:

```text
export JOBNIAC_PDFIUM_DIR="$PWD/vendor/pdfium/mac-arm64"   # macOS, Apple Silicon (mac-x64 on Intel)
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow and the architecture rules.

## Security

Report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md). Do not open a public
issue for them.

## License

JobniacFast is released under the [MIT License](LICENSE). Third-party components keep their own
licences. Each release ships the notices and a software bill of materials generated by the release
process.

## Links

- [CHANGELOG.md](CHANGELOG.md): what changed in each version
- [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- [docs/ops](docs/ops): clean-machine smoke checklist, MCP client registration, release owner checklist