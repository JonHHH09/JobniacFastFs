import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  MatchBatchPageDto,
  MatchHistoryPageDto,
  MatchReportViewDto,
} from "../../ipc/bindings";
import { formatInstant } from "../../lib/format";
import { MatchingPage } from "./MatchingPage";

// Synthetic identifiers and text only.
const PROFILE_ID = "00000000-0000-4000-8000-0000000000a1";
const JOB_ID = "00000000-0000-4000-8000-0000000000b1";
const OTHER_JOB_ID = "00000000-0000-4000-8000-0000000000b2";
const REPORT_ID = "00000000-0000-4000-8000-0000000000d1";
const OTHER_REPORT_ID = "00000000-0000-4000-8000-0000000000d2";

const CONFIGURED = { matching: true, reviews: true };

function view(id: string, options: { stale?: boolean; outcome?: string } = {}): MatchReportViewDto {
  return {
    stale: options.stale ?? false,
    componentsTruncated: false,
    componentsTotalCount: 1,
    evidenceTruncated: false,
    evidenceTotalCount: 1,
    report: {
      id,
      profileId: PROFILE_ID,
      jobId: JOB_ID,
      profileRevision: "2026-01-02T03:04:05Z",
      jobRevision: "2026-01-02T03:04:06Z",
      algorithmVersion: "deterministic-v1",
      outcome: options.outcome ?? "PARTIAL_MATCH",
      createdAt: "2026-01-03T00:00:00Z",
      overallScore: 62,
      confidence: 80,
      blockerMismatch: false,
      components: [
        { component: "TECHNICAL", earnedPoints: 30, availablePoints: 40, status: "PARTIAL" },
      ],
      evidence: [
        {
          component: "TECHNICAL",
          status: "MATCH",
          sourceType: "normalized_skill",
          fact: "rust",
          outcomeChangingDefect: false,
        },
      ],
    },
  };
}

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/** A promise the test settles by hand, to hold an operation open. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/**
 * Serves the commands of one test. The startup capability check is answered first, so the handler
 * only sees the operations the test is about.
 */
function serve(
  handler: (cmd: string, payload: unknown) => unknown,
  capabilities: { matching: boolean; reviews: boolean } = CONFIGURED,
) {
  mockIPC((cmd, payload) =>
    cmd === "match_capabilities_get" ? capabilities : handler(cmd, payload),
  );
}

/** The Rust request object of a command. The UI sends it as JSON text (`requestJson`). */
function requestOf(payload: unknown): unknown {
  return JSON.parse((payload as { requestJson: string }).requestJson);
}

/** Renders the page with a fresh query client, so no capability answer is cached between tests. */
function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MatchingPage />
    </QueryClientProvider>,
  );
}

/** Renders the page and waits until the startup check has enabled the controls. */
async function renderReady() {
  renderPage();
  await waitFor(() => expect(screen.getByTestId("matchingProfileIdInput")).toBeEnabled());
}

function statusOf(testId: string): string {
  return screen.getByTestId(testId).textContent ?? "";
}

function typeInto(testId: string, value: string) {
  fireEvent.change(screen.getByTestId(testId), { target: { value } });
}

function fillPair() {
  typeInto("matchingProfileIdInput", PROFILE_ID);
  typeInto("matchingJobIdInput", JOB_ID);
}

describe("matching inputs and pair comparison", () => {
  it("asks once at startup and keeps the pair action off until both IDs are entered", async () => {
    const invoke = vi.fn(() => view(REPORT_ID));
    serve(invoke);

    await renderReady();

    expect(invoke).not.toHaveBeenCalled();
    expect(screen.getByTestId("analyzeJobMatchButton")).toBeDisabled();
    expect(statusOf("matchingPairHint")).toBe("Pick a profile and a job to compare them.");
    typeInto("matchingProfileIdInput", PROFILE_ID);
    expect(screen.getByTestId("analyzeJobMatchButton")).toBeDisabled();
    typeInto("matchingJobIdInput", JOB_ID);
    expect(screen.getByTestId("analyzeJobMatchButton")).toBeEnabled();
  });

  it("compares a trimmed pair and shows the report, its staleness, outcome and components", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "match_analyze_pair") {
        seen.push(requestOf(payload));
        return view(REPORT_ID);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    typeInto("matchingProfileIdInput", ` ${PROFILE_ID} `);
    typeInto("matchingJobIdInput", JOB_ID);
    fireEvent.click(screen.getByTestId("analyzeJobMatchButton"));

    await waitFor(() => expect(screen.getByTestId("matchReportPanel")).toBeInTheDocument());
    expect(seen).toEqual([{ profileId: PROFILE_ID, jobId: JOB_ID }]);
    expect(statusOf("matchReportStalenessLabel")).toBe(
      "The saved inputs match the versions in this report.",
    );
    expect(statusOf("matchReportScoreLabel")).toBe("Evidence coverage 80 of 100");
    expect(statusOf("matchReportOutcomeLabel")).toBe("Partial match");
    expect(statusOf("matchReportAlgorithmLabel")).toBe("Method: deterministic-v1");
    expect(screen.getByText("Technical skills", { selector: "span.font-medium" })).toBeVisible();
    expect(screen.getByText("30 of 40 points")).toBeInTheDocument();
    expect(statusOf("matchEvidenceFactLabel")).toContain("rust");
    expect(statusOf("matchEvidenceFactLabel")).toContain("Normalized skill");
  });

  it("shows a stale report as stale and an unknown outcome without a label", async () => {
    serve((cmd) => {
      if (cmd === "match_analyze_pair") {
        return view(REPORT_ID, { stale: true, outcome: "SOMETHING_NEW" });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fillPair();
    fireEvent.click(screen.getByTestId("analyzeJobMatchButton"));

    await waitFor(() => expect(screen.getByTestId("matchReportPanel")).toBeInTheDocument());
    expect(statusOf("matchReportStalenessLabel")).toBe(
      "Saved inputs changed. Compare again for a current report; this report stays unchanged.",
    );
    expect(screen.queryByTestId("matchReportOutcomeLabel")).toBeNull();
  });

  it("does not send a second pair request while the first one runs", async () => {
    const held = deferred<MatchReportViewDto>();
    const invoke = vi.fn((_cmd: string, _payload?: unknown) => held.promise);
    serve(invoke);

    await renderReady();
    fillPair();
    fireEvent.click(screen.getByTestId("analyzeJobMatchButton"));
    fireEvent.click(screen.getByTestId("analyzeJobMatchButton"));

    expect(statusOf("matchWorkspaceMessage")).toBe("Comparing the saved profile and job…");
    expect(screen.getByTestId("analyzeJobMatchButton")).toBeDisabled();
    expect(screen.getByTestId("analyzeJobsPageButton")).toBeDisabled();
    // The pickers also look up the names of the two ids: only the comparison itself is counted.
    expect(invoke.mock.calls.filter(([cmd]) => String(cmd).startsWith("match_"))).toHaveLength(1);
    held.resolve(view(REPORT_ID));
    await waitFor(() => expect(screen.getByTestId("matchReportPanel")).toBeInTheDocument());
  });

  it("shows the not-found copy for a missing saved input and keeps a report already shown", async () => {
    serve((cmd) => {
      if (cmd === "match_analyze_pair") {
        return view(REPORT_ID);
      }
      if (cmd === "match_get_report") {
        reject("not_found", "matching.error.notFound");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fillPair();
    fireEvent.click(screen.getByTestId("analyzeJobMatchButton"));
    await waitFor(() => expect(screen.getByTestId("matchReportPanel")).toBeInTheDocument());

    serve((cmd) => {
      if (cmd === "match_get_report") {
        reject("not_found", "matching.error.notFound");
      }
      if (cmd === "match_list_reports") {
        return historyPage([view(OTHER_REPORT_ID)], null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    fireEvent.click(screen.getByTestId("loadMatchReportsButton"));
    await waitFor(() => expect(screen.getByTestId("openMatchReportButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("openMatchReportButton"));

    await waitFor(() =>
      expect(statusOf("matchWorkspaceMessage")).toBe(
        "The saved profile, job, or report is no longer available.",
      ),
    );
    expect(screen.getByTestId("matchReportPanel")).toBeInTheDocument();
  });

  it("shows the unconfigured copy and no action before any click when no database is configured", async () => {
    serve(() => reject("not_configured", "matching.unconfigured"), {
      matching: false,
      reviews: false,
    });

    renderPage();

    await waitFor(() =>
      expect(statusOf("matchWorkspaceMessage")).toBe("Comparisons are not configured."),
    );
    expect(screen.getByTestId("analyzeJobMatchButton")).toBeDisabled();
    expect(screen.getByTestId("analyzeJobsPageButton")).toBeDisabled();
    expect(screen.getByTestId("loadMatchReportsButton")).toBeDisabled();
    expect(screen.queryByTestId("matchingProfileIdInput")).toBeNull();
  });

  it("shows the unconfigured copy when the first action is refused as not configured", async () => {
    serve(() => reject("not_configured", "matching.unconfigured"));

    await renderReady();
    fillPair();
    fireEvent.click(screen.getByTestId("analyzeJobMatchButton"));

    await waitFor(() =>
      expect(statusOf("matchWorkspaceMessage")).toBe("Comparisons are not configured."),
    );
    expect(screen.getByTestId("analyzeJobMatchButton")).toBeDisabled();
    expect(screen.getByTestId("loadMatchReportsButton")).toBeDisabled();
  });

  it("keeps the message in view when another request holds the workspace", async () => {
    serve((cmd) => {
      if (cmd === "match_list_reports") {
        return historyPage([], null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.click(screen.getByTestId("loadMatchReportsButton"));
    await waitFor(() =>
      expect(screen.getByTestId("matchHistoryEmpty")).toHaveTextContent(
        "No comparison reports found.",
      ),
    );

    serve(() => reject("busy", "matching.busy"));
    fireEvent.click(screen.getByTestId("loadMatchReportsButton"));

    await waitFor(() => expect(screen.getByTestId("loadMatchReportsButton")).toBeEnabled());
    expect(screen.getByTestId("matchHistoryEmpty")).toHaveTextContent(
      "No comparison reports found.",
    );
  });

  it("shows the history heading even before any history is loaded", async () => {
    serve(() => historyPage([], null));

    await renderReady();

    expect(screen.getByText("Report history")).toBeInTheDocument();
  });
});

function batchPage(nextCursor: string | null): MatchBatchPageDto {
  return {
    succeeded: [view(REPORT_ID)],
    failed: [{ profileId: PROFILE_ID, jobId: OTHER_JOB_ID }],
    nextCursor,
  };
}

function historyPage(
  reports: MatchReportViewDto[],
  nextCursor: string | null,
): MatchHistoryPageDto {
  return { reports, nextCursor };
}

describe("jobs page comparison", () => {
  it("compares one page, lists the failed job and pages only when asked", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "match_analyze_page") {
        seen.push(requestOf(payload));
        return batchPage("cursor-2");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    typeInto("matchingProfileIdInput", PROFILE_ID);
    fireEvent.click(screen.getByTestId("analyzeJobsPageButton"));

    await waitFor(() =>
      expect(statusOf("matchWorkspaceMessage")).toBe(
        "Compared this page: 1 reports, 1 failed comparisons.",
      ),
    );
    expect(seen).toEqual([{ profileId: PROFILE_ID, cursor: null }]);
    expect(screen.getByText("Reports from this jobs page")).toBeInTheDocument();
    expect(screen.getAllByRole("meter", { name: "Score 62 of 100" })).toHaveLength(1);
    expect(screen.getByText("Comparison failed.")).toBeInTheDocument();
    expect(seen).toHaveLength(1);

    fireEvent.click(screen.getByTestId("nextMatchAnalysisPageButton"));
    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]).toEqual({ profileId: PROFILE_ID, cursor: "cursor-2" });
  });

  it("opens a report from the page by its identifier", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "match_analyze_page") {
        return batchPage(null);
      }
      if (cmd === "match_get_report") {
        seen.push(requestOf(payload));
        return view(REPORT_ID);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    typeInto("matchingProfileIdInput", PROFILE_ID);
    fireEvent.click(screen.getByTestId("analyzeJobsPageButton"));
    await waitFor(() => expect(screen.getByTestId("openBatchMatchReportButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("openBatchMatchReportButton"));

    await waitFor(() => expect(screen.getByTestId("matchReportPanel")).toBeInTheDocument());
    expect(seen).toEqual([{ id: REPORT_ID }]);
    expect(screen.queryByTestId("nextMatchAnalysisPageButton")).toBeNull();
  });

  it("shows a page of failures alone without the batch heading", async () => {
    serve((cmd) => {
      if (cmd === "match_analyze_page") {
        return {
          succeeded: [],
          failed: [{ profileId: PROFILE_ID, jobId: OTHER_JOB_ID }],
          nextCursor: null,
        };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    typeInto("matchingProfileIdInput", PROFILE_ID);
    fireEvent.click(screen.getByTestId("analyzeJobsPageButton"));

    await waitFor(() => expect(screen.getByText("Comparison failed.")).toBeInTheDocument());
    expect(screen.queryByText("Reports from this jobs page")).toBeNull();
  });
});

describe("report history", () => {
  it("loads the history with blank filters sent as null and marks stale rows", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "match_list_reports") {
        seen.push(requestOf(payload));
        return historyPage([view(REPORT_ID, { stale: true }), view(OTHER_REPORT_ID)], "cursor-2");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    typeInto("matchingProfileIdInput", "   ");
    fireEvent.click(screen.getByTestId("loadMatchReportsButton"));

    await waitFor(() => expect(screen.getByText("Inputs changed")).toBeInTheDocument());
    expect(screen.getAllByText("Inputs changed")).toHaveLength(1);
    expect(screen.getAllByText(formatInstant("2026-01-03T00:00:00Z"))).toHaveLength(2);
    expect(seen).toEqual([{ profileId: null, jobId: null, cursor: null }]);
    expect(screen.getByTestId("nextMatchReportsPageButton")).toBeEnabled();
  });

  it("shows the empty copy for a history with no reports", async () => {
    serve(() => historyPage([], null));

    await renderReady();
    fireEvent.click(screen.getByTestId("loadMatchReportsButton"));

    await waitFor(() =>
      expect(screen.getByTestId("matchHistoryEmpty")).toHaveTextContent(
        "No comparison reports found.",
      ),
    );
    expect(screen.queryByTestId("openMatchReportButton")).toBeNull();
  });

  it("keeps the filters of the history when the next page is requested", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "match_list_reports") {
        seen.push(requestOf(payload));
        return seen.length === 1
          ? historyPage([view(REPORT_ID)], "cursor-2")
          : historyPage([view(OTHER_REPORT_ID)], null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    typeInto("matchingJobIdInput", JOB_ID);
    fireEvent.click(screen.getByTestId("loadMatchReportsButton"));
    await waitFor(() => expect(screen.getByTestId("nextMatchReportsPageButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("nextMatchReportsPageButton"));

    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]).toEqual({ profileId: null, jobId: JOB_ID, cursor: "cursor-2" });
  });
});

describe("matching capability check", () => {
  it("explains the disabled form while pending, and a failed check apart from not configured", async () => {
    mockIPC((cmd) => {
      if (cmd === "match_capabilities_get") {
        throw new Error("ipc down");
      }
      return null;
    });

    renderPage();

    expect(statusOf("matchWorkspaceMessage")).toBe("Checking whether comparisons are available…");
    await waitFor(() =>
      expect(statusOf("matchWorkspaceMessage")).toBe(
        "Could not check whether comparisons are available.",
      ),
    );
    expect(screen.getByTestId("matchWorkspaceMessage")).toHaveAttribute("role", "alert");
  });
});
