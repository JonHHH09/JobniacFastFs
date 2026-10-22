# MCP Server

JobniacFast includes a Model Context Protocol (MCP) server. An AI client such as Claude Desktop or Claude Code can use it to read and write the same profiles, jobs and match reports as the desktop app. The server uses the same database file, so both see the same data.

There are two ways to run it:

- **stdio** (`jobniac-cli mcp`): the MCP client starts the process and talks to it over stdin and stdout. This is the normal choice. It opens no network listener.
- **Loopback HTTP** (`jobniac-cli http`): opt-in. The process listens on `127.0.0.1` only and needs a bearer token.

The desktop app can also run the loopback server from Settings (**Run the MCP server**, with a port from 1024 to 65535). It creates a new access token each time the server starts, and **Copy connection details** gives you what to paste into your client.

## Register the server with a client

`jobniac-cli` is installed next to the app. On Windows with the per-user installer it is `%LOCALAPPDATA%\JobniacFast\jobniac-cli.exe`. On macOS it is `/Applications/JobniacFast.app/Contents/MacOS/jobniac-cli`.

Add an entry to the client's `mcpServers` block. Replace `<you>` with your Windows user name. JSON needs each backslash in a Windows path doubled.

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

- Claude Desktop on Windows: put it in `%APPDATA%\Claude\claude_desktop_config.json` and restart the client.
- Claude Code: put it in a `.mcp.json` file in the project folder, then run `claude mcp list`. `jobniac` should show as connected.
- Other clients: use the same `command`, `args` and `env`.

To use a database file other than the default, add `"JOBNIAC_DB_PATH": "<absolute path>"` to `env`. To remove the registration, delete the `jobniac` entry and restart the client.

Check the build with `jobniac-cli --version`, which prints `Jobniac <version>`. The full guide, including a hand check over stdin and the HTTP mode with its token, is in [docs/ops/mcp-client-registration.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/docs/ops/mcp-client-registration.md).

If the database cannot be opened, the server still starts, but every database call fails and `health` reports `DOWN`.

## Opt-in flags that affect tools

A flag is on only when its value is exactly `1`. Pass it in the client's `env` block. Without any flag the server lists 34 tools. All flags together give 40.

| Variable | Effect on tools |
|---|---|
| `JOBNIAC_LINK_FETCH_ENABLED=1` | Adds `add_job_from_link`. |
| `JOBNIAC_ARBEITNOW_ENABLED=1` | Adds `scan_arbeitnow_jobs` and `import_arbeitnow_job`. Also needs `JOBNIAC_ARBEITNOW_CANDIDATE_KEY` and a valid `JOBNIAC_CURSOR_HMAC_SECRET`. |
| `JOBNIAC_DELETION_ENABLED=1` | Adds `delete_profile` and `delete_job`. |
| `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED=1` | Adds `get_document_artifact`. |
| `JOBNIAC_GENERATED_ENABLED=1` | No new tools. The `generate_*` tools start working. Run `jobniac-cli setup-generated-root` once first. |
| `JOBNIAC_DOCUMENT_IMPORT_ROOT` | Not a flag. An absolute folder that `store_document_file` and `extract_pdf_text` read PDFs from. |
| `JOBNIAC_CURSOR_HMAC_SECRET` | Not a flag. Keeps paging cursors valid across restarts. 32 characters or more. |

In HTTP mode, every authenticated session gets the opt-in tools, so enable only the flags you need. See [Data and Privacy](Data-and-Privacy.md).

## Tools

The tool names below are exact.

### Health

- `health`: return the current local health envelope.

### Profiles

- `list_profiles`: list a bounded page of profile identities.
- `get_profile`: get a profile by UUID.
- `create_profile`: create a profile.
- `update_profile`: replace a profile using its latest revision.
- `update_profile_project`: partially update one project of a profile.
- `search_profiles`: search profile fields with bounded, deterministic ranking.
- `delete_profile`: delete a profile (opt-in).

### Jobs

- `list_jobs`: list a bounded page of stored jobs.
- `get_job`: get a stored job by UUID, with source provenance.
- `search_jobs`: search stored jobs with deterministic ranking.
- `add_job_from_text`: save a job from pasted text.
- `add_job_from_link`: save a job by fetching a URL (opt-in).
- `update_job`: partially update a job using its expected revision.
- `delete_job`: delete a stored job (opt-in).
- `scan_arbeitnow_jobs`: read-only scan of the Arbeitnow board (opt-in).
- `import_arbeitnow_job`: import one scan candidate (opt-in).

### Matching and reviews

- `analyze_job_match`: create or reuse an immutable match report for a profile and a job.
- `analyze_all_job_matches`: analyze one bounded page of jobs for a profile.
- `get_match_report`: get a report, with whether its inputs have changed since.
- `list_match_reports`: list reports, with optional profile and job filters.
- `submit_match_review`: save an advisory review and any disagreement.
- `get_match_review`: get one advisory review.
- `list_match_reviews`: list advisory reviews.
- `list_match_disagreements`: list disagreements.
- `acknowledge_match_disagreement`: acknowledge a disagreement.
- `link_match_disagreement`: link a disagreement to an existing issue reference.

### Documents and PDF import

- `store_document_file`: store a PDF from the import root and return its metadata.
- `get_document_metadata`: get stored document metadata by UUID.
- `extract_pdf_text`: preview the text of a local PDF without storing it.
- `extract_stored_pdf_text`: extract text from a stored PDF.
- `ingest_profile_from_stored_pdf`: fill a profile from a stored PDF extraction.
- `get_profile_pdf_source`: get the PDF extraction linked to a profile.

### Generating PDFs

- `generate_pdf_file`: generate a standalone PDF.
- `generate_pdf_resume`: generate the master resume PDF of a profile.
- `generate_canadian_pdf_resume`: generate the Canadian resume PDF.
- `generate_canadian_french_pdf_resume`: generate the French Canadian resume PDF.
- `generate_german_tailored_resume`: generate the job-specific English and German resumes.
- `generate_german_cover_letter`: generate a German cover letter for the selected resume.
- `get_document_artifact`: get a stored PDF up to 1 MiB as metadata and Base64 content (opt-in).

## A stable contract

Tool names, argument keys, result envelopes and error semantics are a stable public contract. Any change to them is recorded in [CHANGELOG.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/CHANGELOG.md) and covered by a test. Text read from PDFs and web pages is returned as untrusted data.

## Related

- [Getting Started](Getting-Started.md)
- [Troubleshooting](Troubleshooting.md)
