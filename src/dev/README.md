# Mock IPC harness (development only)

Views the React UI in a normal browser with realistic synthetic data. `invoke` calls are answered
by in-memory handlers instead of Rust. Nothing here ships: `src/main.tsx` imports it only when
`import.meta.env.DEV && VITE_MOCK_IPC === "1"`, and a production `pnpm build` contains none of it
(check with `grep -r "mockIpc\|example.org" dist/`).

## Run

```
pnpm dev:mock                          # vite --mode mock, reads .env.mock (VITE_MOCK_IPC=1)
pnpm dev:mock --port 1431 --strictPort
```

Open `http://localhost:1420/?section=<id>` (port 1420 by default; use another port when
`pnpm tauri dev` runs). Section ids: `profiles`, `jobs`, `arbeitnow`, `matching`,
`reviews`, `documents`, `artifacts`, `generation`, `health`. Most screens load data after a user
action (Refresh, Search, View PDF). Vite serves the page without Tauri's CSP, so `data:` page images
load.

## URL parameters

Read once at page load (a reload re-seeds all in-memory state).

| Parameter | Effect |
|---|---|
| `mockFail=<command>` | That command rejects with `{ code: "upstream_unavailable", messageId: "mock.failure" }`. Several: `mockFail=a,b` or repeat the parameter. |
| `mockEmpty=1` | Every store starts with no rows (profiles, jobs, reports, reviews, documents). Capabilities stay on. |
| `mockLatency=<ms>` | Artificial latency per call (default 150; `0` turns it off). |

An unknown command logs `[mock-ipc] no mock for command "<name>"` once and rejects with
`{ code: "internal_error", messageId: "" }` (the shape of the generic Rust failure).

## Fixtures

Synthetic only: fictional people and companies, `example.com` / `example.org` addresses.
Stored PDF ids to type into the Documents and Stored PDFs screens:

- `00000000-0000-4000-8000-000600000001` synthetic-cv.pdf (2 pages)
- `00000000-0000-4000-8000-000600000002` synthetic-cover-letter.pdf (1 page)

Profile ids are `00000000-0000-4000-8000-0001000000NN` (NN = 01..08), job ids use kind `0002`,
reports `0003`, reviews `0004`, disagreements `0005`. PDFs generated in a session get random ids and
are shown on the Generate screen.

## Layout

- `mockIpc.ts`: installs `mockIPC` (with event mocking), merges the handler maps, adds latency,
  `mockFail` and the unknown-command behaviour.
- `mocks/<feature>.ts`: one file per feature, each exporting a `HandlerMap`
  (`profiles`, `jobs`, `matching`, `reviews`, `documents`, `artifacts`, `generation`, `profilePdf` (the profile PDF source lookup),
  `arbeitnow`, `health`, `deletion`, `shell`).
- `mocks/util.ts`: `fail()`, `requestOf()`, `uuid()`, `pageOf()`, `seed()` and URL switches.

## Add a handler for a new command

1. Add the command to the Rust side and regenerate `src/ipc/bindings.ts`.
2. In the owning `mocks/<feature>.ts`, add an entry keyed by the snake_case command name:

   ```ts
   my_new_command: (args: Args) => {
     const request = requestOf(args); // for commands that take `requestJson`
     // ...read or change the in-memory state, return the DTO from bindings.ts
     return { ... };
   },
   ```

   Arguments arrive as the camelCase object `invoke` sends (`{ jobId, expectedRevision }`), or as
   `requestJson` text for the JSON-request commands. `i64` values are decimal strings. Return the
   data only; throw with `fail("not_found", "profile.error.notFound")` for a typed error.
3. Seed rows with `seed([...])` so `?mockEmpty=1` works. For a new feature, create a new file and
   spread its map into `handlers` in `mockIpc.ts`.
