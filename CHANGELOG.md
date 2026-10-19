# Changelog

All notable changes to JobniacFast are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow SemVer. Release
drafts take the body of their version section from this file.

## [Unreleased]

### Changed

- User interface: the "Profile from PDF" section is gone; profiles are built from PDFs through the
  MCP tools, which are unchanged. The Profiles screen shows the stored-PDF source of a profile
  built that way, tells the operator what each field accepts (required or optional, kind of value,
  an example), marks invalid values next to their field with the input that is expected and does not
  send a save Rust would refuse, and the profile workspace can be hidden and shown again.
- Storage: the data now lives in an embedded SQLite file instead of a PostgreSQL 18 server. The
  desktop app, the MCP server and `jobniac-cli` open the same file (WAL mode, busy timeout), and
  `jobniac-cli migrate` acts on that file. Tool results, error messages and the health check keep
  their wording. No data is migrated from an existing PostgreSQL database. The refusal texts of
  `jobniac-cli migrate` and `setup-generated-root` without a usable database now name
  `JOBNIAC_DB_PATH` instead of `JOBNIAC_DB_*`.
- User interface: a left navigation sidebar with one section at a time replaces the single long page.
  Sections keep their unsaved drafts when you switch, the active section is kept in the URL,
  and Ctrl or Cmd with 1 to 9 jumps to a section. New visual design with a System, Light and Dark
  theme switch, bundled Schibsted Grotesk font (SIL OFL-1.1) and Lucide icons (ISC).
- HTTP MCP: stdin admission now applies to POST bodies. Session-less calls with an inline
  `_meta` are refused, SDK error answers no longer echo request content, no `Date` header is sent, GET
  streams no longer use request admission, and at the connection cap an idle or silent connection is
  evicted instead of refusing new clients.
- German PDF rendering and document hashing no longer park an async worker.
- Generic PDFs: long titles and rows of wide glyphs no longer run off the page.
- Arbeitnow scans are limited to two at a time; a further scan is refused at once.
- Worker calls get their full budget after waiting for a permit.
- Review score field clamps to 100 and hints on empty or fractional input; the close-refusal notice can
  be dismissed and is announced again for each refusal.
- CLI: `RUST_LOG` module-path directives can no longer reopen rmcp logs; `--help` no longer says that
  cleanup and setup-generated-root are unavailable.

### Added

- In-app updates: Settings has an Updates card ("Check for updates", and a switch to check once after
  startup, off by default and remembered across restarts) and an "Update available" banner with
  "Install and restart" behind a confirmation. Updates come from the signed GitHub releases and are
  verified against the app's public key before they install. A copy installed from a `.deb` links to the
  releases page instead, and a development build reports updates as unavailable. New desktop commands
  `check_for_update`, `install_update`, `update_settings_get` and `update_set_auto_check`; no MCP change.
- Release workflow: versions are set with `release.py bump X.Y.Z` (new `check-versions` check in CI), the
  release job now builds signed updater artifacts (`latest.json` and `.sig` files, needs the repository
  secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`), and the workflow tags
  the merge commit and publishes the release itself after every gate passes (`-dev.N` versions as
  prereleases, never the latest).
- Desktop diagnostics on stderr, a startup refusal message box (Windows) and the opt-in log file
  `JOBNIAC_LOG_FILE=1`.
- `JOBNIAC_DB_PATH`: absolute path of the SQLite database file. Without it the file is
  `%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3` on Windows and
  `$XDG_DATA_HOME/jobniac/data/jobniac.sqlite3` (or `~/.local/share/jobniac/data/jobniac.sqlite3`) on
  Linux. The app and `jobniac-cli` create the folder owner-only (on Unix the database file is `0600`
  too) and bring the schema up to date when they start. The MCP server that the app starts receives the
  database file the app opened as `JOBNIAC_DB_PATH`, so it finds the default location too.
- MCP: the `store_document_file` tool description now says the PDF is stored "in the local database".
  Tool names, arguments and results are unchanged.
- Linux worker children (PDF extraction and rendering, link and Arbeitnow fetches) now confine themselves
  with Landlock and seccomp: no file system access (render may read the system fonts), no sockets for PDF
  workers, no program execution, signals only to themselves. Kernels without Landlock run with seccomp only
  and log a warning.
- Desktop-only `documents_list` command (no MCP tool): the stored-PDF picker lists stored documents with
  size, pages and date, filters by file name, and the review report field picks a saved report by job,
  profile, date and score.
- Settings section with an MCP server switch (off by default, remembered across restarts): the desktop app
  starts and stops the loopback HTTP MCP server (`jobniac-cli http`) as a child process, with a fresh bearer
  token per start that is copied to the clipboard from Rust and never reaches the webview; the child also exits when the app dies for any reason, including a hard kill.

### Removed

- The PostgreSQL requirement: no server, role or database has to be set up, and the PostgreSQL
  adapter, its migrations and its integration tests are gone.
- `JOBNIAC_DB_HOST`, `JOBNIAC_DB_PORT`, `JOBNIAC_DB_NAME`, `JOBNIAC_DB_USER` and
  `JOBNIAC_DB_PASSWORD`. They are ignored; use `JOBNIAC_DB_PATH` to choose another file.

### Fixed

- Window close refused after a reload with stale unsaved flags.
- Arbeitnow next page raced a replacing scan; the artifact page turn could show another document's page;
  the documents and profile-PDF screens now show unavailable on load.
- Stored name `.` or `..` falls back to `stored.pdf`; glossary word keys and size are bounded; a bracketed
  DNS name in a job link URL is refused; the artifact id must be canonical; a stored PDF over 1 MiB is
  refused before it is read for a preview; a missing German personal-details row is not found; a profile
  revision such as `+7` is refused; a post-link generation claim is no longer compensated twice.
- Blank import paths made of U+001C to U+001F are invalid paths.
- Generated-asset publication no longer depends on the umask; the pinned-read chain refuses non-plain
  names; the generated root is created safely under a first-run race and is left untouched when
  `O_TMPFILE` is unsupported; the Windows initialization rollback no longer orphans the capability or
  removes a concurrent lock; the worker process group is killed before the leader is reaped; an expired
  lease refuses before any file write.
- `migrate` and `inspect_schema` refuse while `PGSERVICE` is set.
- A profile search truncation-flag decode failure is an internal error, not database unavailable.

### Performance

- Job and profile search postings are written with one `unnest` insert; job skills and project
  technologies reuse one prepared statement; German resume and cover letter trees are inserted per table.
- Arbeitnow scans skip HTML-to-text unless the query has terms; job search clones only the returned
  rows; batch match analysis canonicalizes profile skills once per page; page PNGs encode faster; a
  winning generated-asset insert skips the redundant read-back and re-hash.

### Security

- Linux worker children get a fixed `PATH` and default signal dispositions; the Environment `Debug`
  output prints counts only; SDK error answers and the rmcp log no longer carry request content.

## [0.1.0] - 2026-10-16

First release of JobniacFast, the Tauri 2 desktop application (Rust core, React and TypeScript user interface).

### Added

- Desktop application (`jobniac`) with profiles, job postings, matching, resume and cover letter
  generation, documents and PDF text extraction and rendering, link capture and the Arbeitnow
  screens, artifacts, deletion and an unsaved-edit close gate.
- `jobniac-cli` console binary: MCP over STDIO and loopback Streamable HTTP (both opt-in),
  migrations and maintenance commands. It ships next to the desktop application.
- Windows NSIS installer (per user, WebView2 bootstrapper) and Linux AppImage and `.deb` (x86-64).
- Release artifacts: CycloneDX SBOMs, `THIRD-PARTY-NOTICES.txt`, payload inventories and
  `SHA256SUMS`.

### Changed

- MCP tool names, argument keys, envelopes and error semantics are a stable public contract.

### Security

- Outbound fetches are rustls-only and use an allow-listed policy; PDF work runs in isolated
  worker processes; opt-in features stay off unless `1` is set.
