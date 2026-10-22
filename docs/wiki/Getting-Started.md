# Getting Started

This page walks through the screens of the desktop app. Install it first: [Installation](Installation.md).

The navigation lists the screens in groups. The app opens on **Profiles**. Press Ctrl+K (Cmd+K on macOS) to search sections, profiles and saved jobs. The theme can be set to System, Light or Dark.

Some features are off until you turn them on with an environment variable (generation, deletion, link fetch, Arbeitnow, export). A screen for such a feature says so when it is unavailable. See [Data and Privacy](Data-and-Privacy.md).

## Candidates

### Profiles

Create, edit and search the candidate profiles you match against. Each field says whether it is required or optional, what kind of value it takes and gives an example. An invalid value is marked next to its field, and a save that would be refused is not sent. The profile workspace can be hidden and shown again. If you have unsaved edits, the app asks before you leave.

A profile can also be built from a PDF resume. The desktop app has no separate screen for this: use the MCP tools (`store_document_file`, then `ingest_profile_from_stored_pdf`), see [MCP Server](MCP-Server.md). The Profiles screen shows the stored PDF a profile was built from.

## Jobs

### Saved jobs

Save job postings, edit them and find them again.

- Paste the text of a posting and choose **Save posting**.
- Enter a public http or https link and choose **Add from link**. This fetches the page, so it needs `JOBNIAC_LINK_FETCH_ENABLED=1`.
- Search saved jobs by title, company, location or skills, and open a job to edit it.

### Arbeitnow

Scan the Arbeitnow job board for postings to save. Filter by search terms, location, tags, job types, remote only and visa sponsorship, then choose **Scan**. **Load more results** reads older postings. **Import** saves a posting as a job. A posting that is already saved is marked. This screen needs `JOBNIAC_ARBEITNOW_ENABLED=1`.

## Matching

### Matches

Pick a saved profile and a saved job and compare them. The result is a saved report with a score out of 100, an outcome (strong match, partial match, weak match or insufficient evidence), points per component and the evidence behind them. Matching is deterministic: the same inputs give the same report. A report notes when the saved profile or job has changed since it was made. You can also compare one profile with one page of jobs, and open the history of saved reports.

### Reviews

Add an advisory review to the match report you opened, and see saved reviews and disagreements. A review is separate from the deterministic report.

## Documents

### PDF documents

Import a PDF from your configured import root (`JOBNIAC_DOCUMENT_IMPORT_ROOT`), preview its extracted text, or open a stored PDF to read its pages. The source file is left in place.

### Generate

Choose a PDF type and generate it:

- Standalone PDF (title, body and file name)
- Master resume
- Canadian resume
- French Canadian resume
- Germany resume (English and German)
- German cover letter, which uses the exact Germany resume you select

Generation needs `JOBNIAC_GENERATED_ENABLED=1` and a one-time `jobniac-cli setup-generated-root`. You can view the pages and a text preview of the result.

### Stored PDFs

Preview the PDFs the app has stored, page by page, and choose **Save PDF...** to export a copy to a new file. Export needs `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED=1`.

## System

- **Health**: checks that the database and the shared services respond. It uses the same check as the MCP `health` tool.
- **Settings**: turns the built-in MCP server on and off and sets its port, and holds the update controls ([Updates](Updates.md)).

## Next

- [Data and Privacy](Data-and-Privacy.md)
- [MCP Server](MCP-Server.md)
- [Troubleshooting](Troubleshooting.md)
