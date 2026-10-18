# Security Policy

## Supported versions

JobniacFast is in early development (0.x). Security fixes go to the latest commit on `main` and to the latest release, once there
is one. Older releases are not supported.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub private vulnerability reporting: open the
repository's **Security** tab and choose **Report a vulnerability**, or go straight to
<https://github.com/JonHHH09/JobniacFastFs/security/advisories/new>.

Published advisories are listed at <https://github.com/JonHHH09/JobniacFastFs/security/advisories>.

Do not open a public issue or pull request for a suspected vulnerability, and do not post exploit
details, tokens, database connection strings or personal data in public.

The project has a single maintainer (@JonHHH09). Reports are handled on a best-effort basis; there
is no guaranteed response or fix time.

## Scope

In scope:

- The Tauri desktop app: IPC commands, capabilities, content security policy and window handling.
- The MCP STDIO server and the opt-in loopback HTTP transport, including bearer authentication.
- Outbound link fetching and the Arbeitnow scan (both opt-in).
- The sandboxed PDF and fetch worker processes.
- Generated-file and import-root handling.
- Handling of the database location (`JOBNIAC_DB_PATH`), secrets and error output.

Out of scope: vulnerabilities in third-party dependencies that are not exploitable through this
project (report them upstream) and issues that require an already compromised machine or account.

## Security model

JobniacFast follows this security model:

- The database is a local SQLite file, created owner-only in the per-user data folder; a custom
  `JOBNIAC_DB_PATH` must be a clean absolute path. No credentials exist and no `.env` file is loaded.
- Errors are sanitized: raw errors, connection strings, secrets and local paths are not shown.
- The database schema changes only through the explicit `migrate` command.
- The STDIO MCP server writes only JSON-RPC to stdout.
- Risky capabilities (link fetching, the Arbeitnow scan, deletion, document artifacts and the
  loopback HTTP transport) are off unless explicitly enabled.
- Untrusted PDFs and web pages are processed in resource-limited worker processes with an
  allowlisted environment.
- The web UI has no filesystem, shell or network permissions; file paths and tokens stay in Rust.
