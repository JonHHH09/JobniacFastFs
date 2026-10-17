import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type {
  ArbeitnowImportResultDto,
  ArbeitnowPageDto,
  ArbeitnowRowDto,
} from "../../ipc/bindings";
import { ArbeitnowPage } from "./ArbeitnowPage";

// Synthetic postings and identifiers only. The row ids are opaque; the token-like values below stand
// in for the candidate token and cursor that Rust keeps. They must never be sent back over IPC.
const CANDIDATE_TOKEN = "synthetic-candidate-token-7f3a9c";
const CURSOR = "synthetic-cursor-value-91bd";
const ROW_A = "row-id-0000000000000000000000000000000a";
const ROW_B = "row-id-0000000000000000000000000000000b";
const ROW_C = "row-id-0000000000000000000000000000000c";
const JOB_ID = "00000000-0000-4000-8000-0000000000d1";

function row(
  rowId: string,
  title: string,
  overrides: Partial<ArbeitnowRowDto> = {},
): ArbeitnowRowDto {
  return {
    rowId,
    title,
    company: "Example Labs",
    location: "Berlin",
    remote: true,
    postedAt: "2026-01-02",
    excerpt: "Synthetic excerpt of the role.",
    tags: ["rust"],
    jobTypes: ["full-time"],
    importStatus: "none",
    ...overrides,
  };
}

const NOTICE = "Upstream results are mutable; this scan is not a snapshot.";

const FIRST_PAGE: ArbeitnowPageDto = {
  rows: [row(ROW_A, "Rust Engineer")],
  hasNextPage: true,
  consistencyNotice: NOTICE,
};

const SECOND_PAGE: ArbeitnowPageDto = {
  rows: [
    row(ROW_B, "Second Engineer", { company: "", remote: false, postedAt: null, excerpt: "" }),
  ],
  hasNextPage: false,
  consistencyNotice: NOTICE,
};

type Call = { cmd: string; payload: unknown };
type Handler = (payload: unknown) => unknown;

/** A backend for the screen. Unknown commands fail, so a stray call is visible. */
function backend(composed: boolean, handlers: Record<string, Handler> = {}) {
  const calls: Call[] = [];
  mockIPC((cmd, payload) => {
    calls.push({ cmd, payload });
    const handler = handlers[cmd];
    if (handler) {
      return handler(payload) as never;
    }
    if (cmd === "arbeitnow_capabilities_get") {
      return { arbeitnow: composed } as never;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  return calls;
}

/** Throws a typed command error the way the generated bindings receive it. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ArbeitnowPage />
    </QueryClientProvider>,
  );
}

/** Waits until Rust has answered the capability and the screen is shown. */
async function openScreen() {
  await screen.findByTestId("arbeitnowPage");
}

async function scanWith(query = "") {
  if (query !== "") {
    fireEvent.change(screen.getByTestId("arbeitnowQuery"), { target: { value: query } });
  }
  fireEvent.click(screen.getByTestId("arbeitnowScanButton"));
}

function statusText(): string | null {
  return screen.queryByTestId("arbeitnowStatusLabel")?.textContent ?? null;
}

/** Every payload sent through invoke, serialized, so a leaked value can be searched for. */
function sentText(calls: Call[]): string {
  return JSON.stringify(calls.map(({ cmd, payload }) => ({ cmd, payload })));
}

describe("ArbeitnowPage visibility", () => {
  it("stays hidden when the service is not composed", async () => {
    const calls = backend(false);
    renderPage();
    await waitFor(() =>
      expect(calls.some(({ cmd }) => cmd === "arbeitnow_capabilities_get")).toBe(true),
    );
    await waitFor(() => expect(screen.queryByTestId("arbeitnowPage")).toBeNull());
    expect(calls.some(({ cmd }) => cmd.startsWith("arbeitnow_scan"))).toBe(false);
  });

  it("is shown when the service is composed", async () => {
    const calls = backend(true);
    renderPage();
    await openScreen();
    expect(calls.some(({ cmd }) => cmd === "arbeitnow_capabilities_get")).toBe(true);
  });

  it("is hidden while the capability is unanswered", () => {
    backend(true);
    renderPage();
    expect(screen.queryByTestId("arbeitnowPage")).toBeNull();
  });

  it("is hidden when the capability command fails", async () => {
    backend(true, {
      arbeitnow_capabilities_get: () => reject("internal_error", "startup.failed"),
    });
    renderPage();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByTestId("arbeitnowPage")).toBeNull();
  });
});

describe("ArbeitnowPage scan form", () => {
  it("shows the idle form with the UI limits and no rows", async () => {
    backend(true);
    renderPage();
    await openScreen();
    expect(screen.getByRole("heading", { name: "Scan Arbeitnow" })).toBeInTheDocument();
    expect(screen.getByTestId("arbeitnowQuery")).toHaveAttribute("maxlength", "256");
    expect(screen.getByTestId("arbeitnowLocation")).toHaveAttribute("maxlength", "128");
    expect(screen.getByTestId("arbeitnowScanButton")).toHaveTextContent("Scan");
    expect(screen.queryByTestId("arbeitnowStatusLabel")).toBeNull();
    expect(screen.queryByTestId("arbeitnowNextButton")).toBeNull();
  });

  it("sends blank text as null, flags only when checked, and tags as typed", async () => {
    const calls = backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
    });
    renderPage();
    await openScreen();
    fireEvent.change(screen.getByTestId("arbeitnowLocation"), { target: { value: "   " } });
    fireEvent.change(screen.getByTestId("arbeitnowTags"), { target: { value: " rust, , go " } });
    fireEvent.click(screen.getByTestId("arbeitnowRemoteOnly"));
    await scanWith("  rust  ");
    await screen.findByTestId("arbeitnowImportButton");
    const scan = calls.find(({ cmd }) => cmd === "arbeitnow_scan");
    expect(scan?.payload).toEqual({
      request: {
        query: "  rust  ",
        location: null,
        remoteOnly: true,
        visaSponsorship: false,
        tags: " rust, , go ",
        jobTypes: "",
      },
    });
  });

  it("scans with the Enter key in the search field", async () => {
    const calls = backend(true, { arbeitnow_scan: () => FIRST_PAGE });
    renderPage();
    await openScreen();
    fireEvent.change(screen.getByTestId("arbeitnowQuery"), { target: { value: "rust" } });
    fireEvent.keyDown(screen.getByTestId("arbeitnowQuery"), { key: "Enter" });
    await screen.findByTestId("arbeitnowImportButton");
    expect(calls.filter(({ cmd }) => cmd === "arbeitnow_scan")).toHaveLength(1);
  });

  it("sends the visa flag when checked", async () => {
    const calls = backend(true, { arbeitnow_scan: () => FIRST_PAGE });
    renderPage();
    await openScreen();
    fireEvent.click(screen.getByTestId("arbeitnowVisa"));
    await scanWith();
    await screen.findByTestId("arbeitnowImportButton");
    const scan = calls.find(({ cmd }) => cmd === "arbeitnow_scan");
    expect(scan?.payload).toMatchObject({ request: { visaSponsorship: true, query: null } });
  });
});

describe("ArbeitnowPage results", () => {
  it("shows the rows with the joined subtitle, excerpt and notice", async () => {
    backend(true, { arbeitnow_scan: () => FIRST_PAGE });
    renderPage();
    await openScreen();
    await scanWith();
    expect(await screen.findByText("Rust Engineer")).toBeInTheDocument();
    expect(screen.getByText("Example Labs")).toBeInTheDocument();
    expect(screen.getByText("Berlin")).toBeInTheDocument();
    expect(screen.getByText("Remote")).toBeInTheDocument();
    // The job type filter offers "Full time" as a choice too: look at the row's badge.
    const resultRow = screen.getAllByTestId("arbeitnowRowTitle")[0]?.closest("li");
    expect(within(resultRow as HTMLElement).getByText("Full time")).toBeInTheDocument();
    expect(screen.getByText("Jan 2, 2026")).toBeInTheDocument();
    expect(screen.getByText("Synthetic excerpt of the role.")).toBeInTheDocument();
    expect(screen.getByTestId("arbeitnowNoticeLabel")).toHaveTextContent(NOTICE);
    expect(screen.getByTestId("arbeitnowImportButton")).toHaveAccessibleName(
      "Import Rust Engineer",
    );
  });

  it("omits the empty parts of the subtitle", async () => {
    backend(true, {
      arbeitnow_scan: () => ({ ...SECOND_PAGE, hasNextPage: false }),
    });
    renderPage();
    await openScreen();
    await scanWith();
    expect(await screen.findByText("Second Engineer")).toBeInTheDocument();
    expect(screen.getByText("Berlin")).toBeInTheDocument();
  });

  it("reports an empty scan with its copy", async () => {
    backend(true, {
      arbeitnow_scan: () => ({ rows: [], hasNextPage: false, consistencyNotice: "" }),
    });
    renderPage();
    await openScreen();
    await scanWith();
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "No matching Arbeitnow jobs found.",
    );
    expect(screen.queryByTestId("arbeitnowNoticeLabel")).toBeNull();
  });

  it("locks the controls and shows the progress copy while a scan runs", async () => {
    let finish: (page: ArbeitnowPageDto) => void = () => undefined;
    backend(true, {
      arbeitnow_scan: () => new Promise<ArbeitnowPageDto>((resolve) => (finish = resolve)) as never,
    });
    renderPage();
    await openScreen();
    await scanWith("rust");
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "Scanning Arbeitnow…",
    );
    expect(screen.getByTestId("arbeitnowScanButton")).toBeDisabled();
    expect(screen.getByTestId("arbeitnowScanButton")).toHaveTextContent("Scanning…");
    expect(screen.getByTestId("arbeitnowQuery")).toBeDisabled();
    expect(screen.getByTestId("arbeitnowRemoteOnly")).toBeDisabled();
    finish(FIRST_PAGE);
    await screen.findByTestId("arbeitnowImportButton");
    expect(screen.getByTestId("arbeitnowScanButton")).toBeEnabled();
    expect(screen.getByTestId("arbeitnowQuery")).toBeEnabled();
  });

  it("does not start a second scan while one runs", async () => {
    let finish: (page: ArbeitnowPageDto) => void = () => undefined;
    const calls = backend(true, {
      arbeitnow_scan: () => new Promise<ArbeitnowPageDto>((resolve) => (finish = resolve)) as never,
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(screen.getByTestId("arbeitnowScanButton"));
    fireEvent.keyDown(screen.getByTestId("arbeitnowQuery"), { key: "Enter" });
    expect(calls.filter(({ cmd }) => cmd === "arbeitnow_scan")).toHaveLength(1);
    finish(FIRST_PAGE);
    await screen.findByTestId("arbeitnowImportButton");
  });
});

describe("ArbeitnowPage import", () => {
  it("imports one row by its opaque id and disables its button after the import", async () => {
    const result: ArbeitnowImportResultDto = {
      rowId: ROW_A,
      status: "createdJob",
      jobId: JOB_ID,
    };
    const calls = backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_import_row: () => result,
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowImportButton"));
    await waitFor(() =>
      expect(screen.getByTestId("arbeitnowImportButton")).toHaveTextContent("Imported"),
    );
    expect(screen.getByTestId("arbeitnowImportButton")).toBeDisabled();
    expect(screen.getByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "Job imported from Arbeitnow.",
    );
    const imports = calls.filter(({ cmd }) => cmd === "arbeitnow_import_row");
    expect(imports).toEqual([{ cmd: "arbeitnow_import_row", payload: { rowId: ROW_A } }]);
  });

  it("shows the already-saved copy for a reused posting", async () => {
    backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_import_row: () => ({ rowId: ROW_A, status: "reusedExistingJob", jobId: JOB_ID }),
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowImportButton"));
    await waitFor(() =>
      expect(screen.getByTestId("arbeitnowImportButton")).toHaveTextContent("Already saved"),
    );
    expect(screen.getByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "This posting was already saved.",
    );
  });

  it("shows the import progress copy and locks the controls during an import", async () => {
    let finish: (result: ArbeitnowImportResultDto) => void = () => undefined;
    backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_import_row: () =>
        new Promise<ArbeitnowImportResultDto>((resolve) => (finish = resolve)) as never,
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowImportButton"));
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "Importing the selected job…",
    );
    expect(screen.getByTestId("arbeitnowImportButton")).toBeDisabled();
    expect(screen.getByTestId("arbeitnowScanButton")).toBeDisabled();
    finish({ rowId: ROW_A, status: "createdJob", jobId: JOB_ID });
    await waitFor(() => expect(screen.getByTestId("arbeitnowImportButton")).toBeDisabled());
  });

  it("shows the candidate copy for a refused import and keeps the row importable", async () => {
    let attempts = 0;
    backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_import_row: () => {
        attempts += 1;
        if (attempts === 1) {
          reject("validation_error", "arbeitnow.error.import.validation");
        }
        return { rowId: ROW_A, status: "createdJob", jobId: JOB_ID };
      },
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowImportButton"));
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "This candidate is invalid or has expired. Scan again to import it.",
    );
    expect(screen.getByTestId("arbeitnowImportButton")).toBeEnabled();
    fireEvent.click(screen.getByTestId("arbeitnowImportButton"));
    await waitFor(() =>
      expect(screen.getByTestId("arbeitnowImportButton")).toHaveTextContent("Imported"),
    );
  });

  it("shows the generic import copy for an unknown failure", async () => {
    backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_import_row: () => reject("internal_error", ""),
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowImportButton"));
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "Unable to import this job. Please try again.",
    );
  });
});

describe("ArbeitnowPage next page", () => {
  it("appends the next rows, then hides the button when no page follows", async () => {
    const calls = backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_next_page: () => SECOND_PAGE,
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowNextButton"));
    expect(await screen.findByText("Second Engineer")).toBeInTheDocument();
    expect(screen.getAllByTestId("arbeitnowRowTitle")).toHaveLength(2);
    expect(screen.queryByTestId("arbeitnowNextButton")).toBeNull();
    const next = calls.find(({ cmd }) => cmd === "arbeitnow_next_page");
    // Rust keeps the cursor and the filters: the command carries no argument.
    expect(next?.payload).toEqual({});
  });

  it("keeps the rows and their import status after a failed next page", async () => {
    backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_next_page: () => reject("validation_error", "arbeitnow.error.next.validation"),
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowNextButton"));
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "These results are no longer continuable. Run the scan again.",
    );
    expect(screen.getAllByTestId("arbeitnowRowTitle")).toHaveLength(1);
    expect(screen.getByTestId("arbeitnowNextButton")).toBeEnabled();
  });

  it("disables the next button while a page loads", async () => {
    let finish: (page: ArbeitnowPageDto) => void = () => undefined;
    backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_next_page: () =>
        new Promise<ArbeitnowPageDto>((resolve) => (finish = resolve)) as never,
    });
    renderPage();
    await openScreen();
    await scanWith();
    fireEvent.click(await screen.findByTestId("arbeitnowNextButton"));
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "Loading more Arbeitnow results…",
    );
    expect(screen.getByTestId("arbeitnowNextButton")).toBeDisabled();
    finish(SECOND_PAGE);
    await screen.findByText("Second Engineer");
  });
});

describe("ArbeitnowPage failures", () => {
  it("maps each scan failure to its copy and keeps the rows of an earlier scan", async () => {
    const failures: Array<[string, string, string]> = [
      [
        "validation_error",
        "arbeitnow.error.scan.validation",
        "Check the scan filters and try again.",
      ],
      [
        "upstream_rate_limited",
        "arbeitnow.error.scan.rateLimited",
        "Arbeitnow is rate limiting requests. Wait a moment and scan again.",
      ],
      [
        "upstream_unavailable",
        "arbeitnow.error.scan.unavailable",
        "Arbeitnow is temporarily unavailable. Try again later.",
      ],
      [
        "upstream_invalid_response",
        "arbeitnow.error.scan.invalidResponse",
        "Arbeitnow returned data that could not be used.",
      ],
    ];
    for (const [code, messageId, copy] of failures) {
      let first = true;
      backend(true, {
        arbeitnow_scan: () => {
          if (first) {
            first = false;
            return FIRST_PAGE;
          }
          return reject(code, messageId);
        },
      });
      const { unmount } = renderPage();
      await openScreen();
      await scanWith();
      await screen.findByTestId("arbeitnowImportButton");
      await scanWith();
      expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(copy);
      expect(screen.getAllByTestId("arbeitnowRowTitle")).toHaveLength(1);
      unmount();
      await waitFor(() => expect(screen.queryByTestId("arbeitnowPage")).toBeNull());
    }
  });

  it("shows the generic scan copy for a failure without a copy id", async () => {
    backend(true, { arbeitnow_scan: () => reject("internal_error", "") });
    renderPage();
    await openScreen();
    await scanWith();
    expect(await screen.findByTestId("arbeitnowStatusLabel")).toHaveTextContent(
      "Unable to scan Arbeitnow. Please try again.",
    );
  });

  it("shows no copy for a busy refusal", async () => {
    backend(true, { arbeitnow_scan: () => reject("busy", "arbeitnow.busy") });
    renderPage();
    await openScreen();
    await scanWith();
    await waitFor(() => expect(screen.getByTestId("arbeitnowScanButton")).toBeEnabled());
    expect(statusText()).toBeNull();
  });
});

describe("ArbeitnowPage IPC payloads", () => {
  it("never sends a candidate token, a cursor or a URL through invoke", async () => {
    const calls = backend(true, {
      arbeitnow_scan: () => FIRST_PAGE,
      arbeitnow_next_page: () => SECOND_PAGE,
      arbeitnow_import_row: () => ({ rowId: ROW_A, status: "createdJob", jobId: JOB_ID }),
    });
    renderPage();
    await openScreen();
    await scanWith("rust");
    fireEvent.click(await screen.findByTestId("arbeitnowNextButton"));
    await screen.findByText("Second Engineer");
    fireEvent.click(screen.getAllByTestId("arbeitnowImportButton")[0]);
    await waitFor(() =>
      expect(screen.getAllByTestId("arbeitnowImportButton")[0]).toHaveTextContent("Imported"),
    );
    const sent = sentText(calls);
    expect(sent).not.toContain(CANDIDATE_TOKEN);
    expect(sent).not.toContain(CURSOR);
    expect(sent).not.toMatch(/candidateToken|nextCursor|cursor|\burl\b/i);
  });

  it("imports the row of its own card, not the first one", async () => {
    const page: ArbeitnowPageDto = {
      rows: [row(ROW_A, "First"), row(ROW_C, "Third")],
      hasNextPage: false,
      consistencyNotice: "",
    };
    const calls = backend(true, {
      arbeitnow_scan: () => page,
      arbeitnow_import_row: () => ({ rowId: ROW_C, status: "createdJob", jobId: JOB_ID }),
    });
    renderPage();
    await openScreen();
    await scanWith();
    const buttons = await screen.findAllByTestId("arbeitnowImportButton");
    const card = buttons[1].closest<HTMLElement>("li");
    expect(card).not.toBeNull();
    if (card) {
      expect(within(card).getByTestId("arbeitnowRowTitle")).toHaveTextContent("Third");
    }
    fireEvent.click(buttons[1]);
    await waitFor(() => expect(buttons[1]).toHaveTextContent("Imported"));
    expect(calls.filter(({ cmd }) => cmd === "arbeitnow_import_row")[0]?.payload).toEqual({
      rowId: ROW_C,
    });
  });
});
