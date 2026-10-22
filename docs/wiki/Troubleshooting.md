# Troubleshooting

## Windows says "Windows protected your PC". Is the installer safe?

The Windows installer is not code-signed yet, so SmartScreen warns about every download. Check the file against `SHA256SUMS` first (see [Installation](Installation.md)). If the hashes match, click **More info**, then **Run anyway**.

## macOS says it cannot verify the app

Releases are ad-hoc signed until a Developer ID is set up, so Gatekeeper blocks the first launch. After you have checked the hash, open System Settings, Privacy & Security, and choose **Open Anyway** for JobniacFast. Or run `xattr -dr com.apple.quarantine /Applications/JobniacFast.app`. The release notes say whether a release is signed with a Developer ID.

## The window is blank on Linux (NVIDIA or Wayland)

On some NVIDIA and Wayland setups the web view draws nothing. Start the app with the DMA-BUF renderer turned off:

```text
WEBKIT_DISABLE_DMABUF_RENDERER=1 ./JobniacFast_<version>_amd64.AppImage
```

For the `.deb`, set the same variable in the environment that starts the installed app.

## The AppImage or .deb does not start on Linux

The packages are built on Ubuntu 24.04, so the system needs Ubuntu 24.04 or newer, or a distribution with an equal or newer glibc. Make an AppImage executable with `chmod +x`.

## PDF import, preview or generation fails

If the PDF library is missing, a PDF action fails with a general error, and a terminal launch logs the warning "the PDF library is not available". The app looks for the PDFium library in its install folder (on macOS, in `Contents/Frameworks` of the app). It is part of every release. If it is missing, the install is incomplete or damaged: check the download against `SHA256SUMS` and reinstall. `JOBNIAC_PDFIUM_DIR`, when set, overrides the location, so unset it if it points to the wrong folder.

If you build from source, fetch PDFium first with `scripts/fetch-pdfium.sh` (see [Development Setup](Development-Setup.md)).

## A screen says a feature is not configured or unavailable

Several features are off until you set a variable to exactly `1`: `true`, `yes` and `01` do not count. See the table in [Data and Privacy](Data-and-Privacy.md). Quit the app and start it again after you change the environment.

- Generate: `JOBNIAC_GENERATED_ENABLED=1`, and run `jobniac-cli setup-generated-root` once.
- Arbeitnow: `JOBNIAC_ARBEITNOW_ENABLED=1`, plus `JOBNIAC_ARBEITNOW_CANDIDATE_KEY` and a `JOBNIAC_CURSOR_HMAC_SECRET` of 32 characters or more.
- Add from link: `JOBNIAC_LINK_FETCH_ENABLED=1`.
- Delete: `JOBNIAC_DELETION_ENABLED=1`.
- Save PDF (export): `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED=1`.
- Import PDF: `JOBNIAC_DOCUMENT_IMPORT_ROOT` set to an absolute folder.

## The app shows a startup message and exits, or Health reports DOWN

The message is "Jobniac could not start. Check local configuration." The text never names the setting. Check the variables you set, in particular `JOBNIAC_DB_PATH`, which must be a clean absolute file path.

If the database file cannot be opened, the server still starts but database calls fail and `health` reports `DOWN`. Common causes are a relative `JOBNIAC_DB_PATH` and a file written by a newer version of the app.

## The database file was written by a newer version

A database file written by a newer version of the app is refused. Update the app to the newer version. Do not copy the file back to an older version.

## The HTTP MCP server does not start

`jobniac-cli http` exits with code 1 if the port or token is invalid: the port must be 1024 to 65535, and the token 43 to 128 characters from `A-Z a-z 0-9 - _`. It exits with code 3 if the port is in use. See [MCP Server](MCP-Server.md).

## The Arbeitnow scan finds nothing

The status says "No matches in the postings checked so far" when older postings remain. Choose **Load more results** to read older postings. If the board is rate limited or unavailable, try again later.

## "Check for updates" does nothing, or says updates are unavailable

A development build and a `.deb` install cannot update themselves. For a `.deb`, download the new package from the releases page. See [Updates](Updates.md).

## Where are the logs?

On a terminal launch the app writes warnings to stderr (`RUST_LOG` changes the level). A Windows release build has no console. To keep a log file, set `JOBNIAC_LOG_FILE=1`. The app writes `jobniac.log` in its application log folder. The file is emptied at startup once it grows past 256 KiB.

## Still stuck

Search the [issues](https://github.com/JonHHH09/JobniacFastFs/issues). Report a security problem privately, as described in [SECURITY.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/SECURITY.md).
