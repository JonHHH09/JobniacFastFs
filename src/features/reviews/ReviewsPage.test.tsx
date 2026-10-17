import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type {
  DisagreementViewDto,
  MatchReportViewDto,
  ReviewSubmitResultDto,
  ReviewViewDto,
} from "../../ipc/bindings";
import { OpenedReportProvider } from "../matching/openedReport";
import { ReviewsPage } from "./ReviewsPage";

// Synthetic identifiers and text only.
const REPORT_ID = "00000000-0000-4000-8000-0000000000d1";
const REVIEW_ID = "00000000-0000-4000-8000-0000000000e1";
const DISAGREEMENT_ID = "00000000-0000-4000-8000-0000000000f1";

const CONFIGURED = { matching: true, reviews: true };

/** The `review_set_draft_dirty` reports the page sent, in order. */
let dirtyReports: boolean[] = [];

beforeEach(() => {
  dirtyReports = [];
  // A test that does not serve its own commands fails on any command it did not expect.
  serve(() => {
    throw new Error("unexpected command");
  });
});

function reviewView(): ReviewViewDto {
  return {
    id: REVIEW_ID,
    reportId: REPORT_ID,
    reviewer: "synthetic-reviewer",
    model: "synthetic-model",
    reviewVersion: "v1",
    outcome: "WEAK_MATCH",
    summary: "score_adjustment",
    createdAt: "2026-01-03T00:00:00Z",
    overallScore: 55,
    blockerMismatch: true,
    components: [
      { component: "TECHNICAL", earnedPoints: 20, availablePoints: 40, status: "MISMATCH" },
    ],
    evidence: [],
    componentsTotalCount: 1,
    componentsTruncated: false,
    evidenceTotalCount: 0,
    evidenceTruncated: false,
  };
}

function disagreementView(overrides: Partial<DisagreementViewDto> = {}): DisagreementViewDto {
  return {
    id: DISAGREEMENT_ID,
    reportId: REPORT_ID,
    reviewId: REVIEW_ID,
    policyVersion: "divergence-v1",
    status: "OPEN",
    createdAt: "2026-01-03T00:00:00Z",
    updatedAt: "2026-01-03T00:00:00Z",
    issueReference: null,
    reasons: ["OVERALL_DELTA"],
    evidenceDefectCodes: ["structured_evidence_missing"],
    reasonsTotalCount: 1,
    reasonsTruncated: false,
    defectsTotalCount: 1,
    defectsTruncated: false,
    ...overrides,
  };
}

/** An opened deterministic report, whose component weights bound the component details. */
function openedReport(): MatchReportViewDto {
  return {
    stale: false,
    componentsTruncated: false,
    componentsTotalCount: 1,
    evidenceTruncated: false,
    evidenceTotalCount: 0,
    report: {
      id: REPORT_ID,
      profileId: "00000000-0000-4000-8000-0000000000a1",
      jobId: "00000000-0000-4000-8000-0000000000b1",
      profileRevision: "2026-01-02T03:04:05Z",
      jobRevision: "2026-01-02T03:04:06Z",
      algorithmVersion: "deterministic-v1",
      outcome: "PARTIAL_MATCH",
      createdAt: "2026-01-03T00:00:00Z",
      overallScore: 62,
      confidence: 80,
      blockerMismatch: false,
      components: [
        { component: "TECHNICAL", earnedPoints: 30, availablePoints: 40, status: "PARTIAL" },
      ],
      evidence: [],
    },
  };
}

function submitResult(disagreement: DisagreementViewDto | null): ReviewSubmitResultDto {
  return { review: reviewView(), disagreement };
}

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/**
 * Serves the commands of one test. The startup capability check is answered first, and the draft
 * dirty reports are recorded, so the handler only sees the operations the test is about.
 */
function serve(
  handler: (cmd: string, payload: unknown) => unknown,
  capabilities: { matching: boolean; reviews: boolean } = CONFIGURED,
) {
  mockIPC((cmd, payload) => {
    if (cmd === "match_capabilities_get") {
      return capabilities;
    }
    if (cmd === "review_set_draft_dirty") {
      dirtyReports.push((payload as { dirty: boolean }).dirty);
      return null;
    }
    return handler(cmd, payload);
  });
}

/** The Rust request object of a command. The UI sends it as JSON text (`requestJson`). */
function requestOf(payload: unknown): unknown {
  return JSON.parse((payload as { requestJson: string }).requestJson);
}

function renderPage(report: MatchReportViewDto | null = null) {
  // A fresh query client, so no capability answer is cached between tests.
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <OpenedReportProvider initialReport={report}>
        <ReviewsPage />
      </OpenedReportProvider>
    </QueryClientProvider>,
  );
}

/** Renders the page and waits until the startup check has enabled the editor. */
async function renderReady(report: MatchReportViewDto | null = null) {
  renderPage(report);
  await waitFor(() => expect(screen.getByLabelText("Reviewer label")).toBeEnabled());
}

function typeInto(testId: string, value: string) {
  fireEvent.change(screen.getByTestId(testId), { target: { value } });
}

/** Fills the four identity fields through their labels. */
function fillIdentityFields(reviewer = " synthetic-reviewer ") {
  fireEvent.change(screen.getByLabelText("Saved report"), { target: { value: ` ${REPORT_ID} ` } });
  fireEvent.change(screen.getByLabelText("Reviewer label"), { target: { value: reviewer } });
  fireEvent.change(screen.getByLabelText("Model label"), { target: { value: "synthetic-model" } });
  fireEvent.change(screen.getByLabelText("Review version"), { target: { value: "v1" } });
}

function statusOf(testId: string): string {
  return screen.getByTestId(testId).textContent ?? "";
}

describe("advisory review draft", () => {
  it("keeps save off until the four identity fields are filled, then sends the trimmed draft", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_submit") {
        seen.push(requestOf(payload));
        return submitResult(null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    expect(screen.getByTestId("submitMatchReviewButton")).toBeDisabled();
    fillIdentityFields();
    expect(screen.getByTestId("submitMatchReviewButton")).toBeEnabled();
    fireEvent.click(screen.getByTestId("submitMatchReviewButton"));

    await waitFor(() => expect(statusOf("reviewStatusLabel")).toBe("Advisory review saved."));
    expect(seen).toEqual([
      {
        reportId: REPORT_ID,
        reviewer: "synthetic-reviewer",
        model: "synthetic-model",
        reviewVersion: "v1",
        overallScore: 0,
        outcome: "INSUFFICIENT_EVIDENCE",
        summary: "review_consistent",
        blockerMismatch: false,
        components: [],
        evidence: [],
      },
    ]);
    expect(screen.getByTestId("savedReviewPanel")).toBeInTheDocument();
    expect(screen.getByText("synthetic-reviewer, version v1")).toBeInTheDocument();
    expect(screen.getByText("The review reports a hard requirement mismatch.")).toBeInTheDocument();
    // Saved, so the draft is clean again and discard is off.
    expect(screen.getByTestId("discardReviewDraftButton")).toBeDisabled();
  });

  it("shows the created disagreement after a save and records its issue reference", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_submit") {
        return submitResult(disagreementView());
      }
      if (cmd === "review_link_disagreement") {
        seen.push(requestOf(payload));
        return disagreementView({ status: "LINKED", issueReference: "SYN-9" });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fillIdentityFields();
    fireEvent.click(screen.getByTestId("submitMatchReviewButton"));

    await waitFor(() => expect(screen.getByTestId("disagreementPanel")).toBeInTheDocument());
    expect(statusOf("disagreementStatusLabel")).toBe("Open");
    expect(statusOf("disagreementReasonsLabel")).toBe("Overall scores differ");
    expect(statusOf("disagreementDefectCodesLabel")).toBe("Structured evidence is missing");
    expect(screen.getByTestId("existingIssueReferenceInput")).toHaveValue("");
    expect(screen.getByTestId("linkMatchDisagreementButton")).toBeDisabled();

    typeInto("existingIssueReferenceInput", " SYN-9 ");
    fireEvent.click(screen.getByTestId("linkMatchDisagreementButton"));

    await waitFor(() => expect(statusOf("disagreementStatusLabel")).toBe("Linked to an issue"));
    expect(seen).toEqual([{ disagreementId: DISAGREEMENT_ID, issueReference: "SYN-9" }]);
  });

  it("sends a component detail with the weight of the opened report", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_submit") {
        seen.push(requestOf(payload));
        return submitResult(null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady(openedReport());
    fireEvent.click(screen.getByTestId("addComponentDetailButton"));
    fireEvent.change(screen.getByLabelText("Earned points"), { target: { value: "30" } });
    fillIdentityFields();
    fireEvent.click(screen.getByTestId("submitMatchReviewButton"));

    await waitFor(() => expect(seen).toHaveLength(1));
    const request = seen[0] as { components: unknown[] };
    expect(request.components).toEqual([
      { component: "TECHNICAL", earnedPoints: 30, availablePoints: 40, status: "UNKNOWN" },
    ]);
  });

  it("keeps the earned points to whole numbers within the weight of the component", async () => {
    await renderReady(openedReport());
    fireEvent.click(screen.getByTestId("addComponentDetailButton"));
    const earned = screen.getByLabelText("Earned points");
    expect(earned).toHaveAttribute("step", "1");

    fireEvent.change(earned, { target: { value: "2.5" } });
    expect(earned).toHaveValue(0);
    fireEvent.change(earned, { target: { value: "90" } });
    expect(earned).toHaveValue(40);
  });

  it("needs an opened report before a component detail can be added", async () => {
    await renderReady();
    expect(screen.getByTestId("addComponentDetailButton")).toBeDisabled();
    expect(
      screen.getByText("Open a deterministic report to add component details."),
    ).toBeInTheDocument();
  });

  it("discard is enabled only for a changed draft and resets it to the defaults", async () => {
    await renderReady();
    expect(screen.getByTestId("discardReviewDraftButton")).toBeDisabled();
    fillIdentityFields();
    expect(screen.getByTestId("discardReviewDraftButton")).toBeEnabled();

    fireEvent.click(screen.getByTestId("discardReviewDraftButton"));

    expect(screen.getByLabelText("Reviewer label")).toHaveValue("");
    expect(screen.getByTestId("discardReviewDraftButton")).toBeDisabled();
  });

  it("reports the dirty draft to Rust, and reports it clean again after a discard", async () => {
    serve((cmd) => {
      if (cmd === "review_submit") {
        return submitResult(null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    expect(dirtyReports).toEqual([]);
    fillIdentityFields();
    await waitFor(() => expect(dirtyReports).toEqual([true]));

    fireEvent.click(screen.getByTestId("discardReviewDraftButton"));
    await waitFor(() => expect(dirtyReports).toEqual([true, false]));
  });

  it("resets the draft after a save, so a discard then has nothing to restore", async () => {
    serve((cmd) => {
      if (cmd === "review_submit") {
        return submitResult(null);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fillIdentityFields();
    fireEvent.click(screen.getByTestId("submitMatchReviewButton"));
    await waitFor(() => expect(statusOf("reviewStatusLabel")).toBe("Advisory review saved."));
    typeInto("reviewreviewerInput", "another-reviewer");
    expect(screen.getByTestId("discardReviewDraftButton")).toBeEnabled();

    fireEvent.click(screen.getByTestId("discardReviewDraftButton"));

    expect(screen.getByTestId("reviewreviewerInput")).toHaveValue("");
    await waitFor(() => expect(dirtyReports[dirtyReports.length - 1]).toBe(false));
  });

  it("keeps the draft and shows the generic copy when a save fails", async () => {
    serve(() => reject("validation_error", "reviews.error.generic"));

    await renderReady();
    fillIdentityFields();
    fireEvent.click(screen.getByTestId("submitMatchReviewButton"));

    await waitFor(() =>
      expect(statusOf("reviewStatusLabel")).toBe(
        "Unable to complete this review request. Your draft is kept.",
      ),
    );
    expect(screen.getByLabelText("Reviewer label")).toHaveValue(" synthetic-reviewer ");
    expect(screen.getByTestId("discardReviewDraftButton")).toBeEnabled();
  });

  it("keeps the message in view when another review request holds the workspace", async () => {
    serve((cmd) => {
      if (cmd === "review_list") {
        return { reviews: [], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fillIdentityFields();
    fireEvent.click(screen.getByTestId("loadReviewHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("loadReviewHistoryButton")).toBeEnabled());

    serve(() => reject("busy", "reviews.busy"));
    fillIdentityFields("another-reviewer");
    fireEvent.click(screen.getByTestId("submitMatchReviewButton"));
    await waitFor(() => expect(screen.getByTestId("submitMatchReviewButton")).toBeEnabled());
    expect(screen.getByLabelText("Reviewer label")).toHaveValue("another-reviewer");
    expect(
      screen.queryByText("Unable to complete this review request. Your draft is kept."),
    ).toBeNull();
  });

  it("shows the unconfigured copy before any click, and disables the editor and hides the lists", async () => {
    serve(() => reject("not_configured", "reviews.unconfigured"), {
      matching: false,
      reviews: false,
    });

    renderPage();

    await waitFor(() =>
      expect(statusOf("reviewStatusLabel")).toBe("Advisory reviews are not configured."),
    );
    expect(screen.getByLabelText("Reviewer label")).toBeDisabled();
    expect(screen.getByTestId("submitMatchReviewButton")).toBeDisabled();
    expect(screen.queryByTestId("reviewListsPanel")).toBeNull();
  });

  it("exposes the object names as test hooks", async () => {
    await renderReady();
    expect(screen.getByTestId("reviewAuthorityLabel")).toBeInTheDocument();
    for (const testId of [
      "reviewreportIdInput",
      "reviewreviewerInput",
      "reviewmodelInput",
      "reviewreviewVersionInput",
      "reviewScoreInput",
      "reviewOutcomeInput",
      "reviewSummaryInput",
    ]) {
      expect(screen.getByTestId(testId)).toBeInTheDocument();
    }
  });

  it("uses the group titles and checkbox copy for the optional rows", async () => {
    await renderReady();
    expect(screen.getByText("Optional component details")).toBeInTheDocument();
    expect(screen.getByText("Optional evidence concern")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("addEvidenceConcernButton"));
    expect(screen.getByText("This concern changes the outcome")).toBeInTheDocument();
    expect(screen.queryByText("Reviews")).toBeNull();
    expect(screen.queryByText("Disagreements")).toBeNull();
  });
});

describe("saved reviews and disagreements", () => {
  it("lists reviews for the report ID filter and opens one", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_list") {
        seen.push(requestOf(payload));
        return { reviews: [reviewView()], nextCursor: null };
      }
      if (cmd === "review_get") {
        seen.push(requestOf(payload));
        return reviewView();
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.change(screen.getByLabelText("Saved report"), { target: { value: REPORT_ID } });
    fireEvent.click(screen.getByTestId("loadReviewHistoryButton"));

    await waitFor(() =>
      expect(
        screen.getByRole("meter", { name: "synthetic-reviewer, advisory score 55 of 100" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByTestId("openReviewButton"));

    await waitFor(() => expect(screen.getByTestId("savedReviewPanel")).toBeInTheDocument());
    expect(seen).toEqual([{ reportId: REPORT_ID, cursor: null }, { id: REVIEW_ID }]);
    expect(screen.getByText("20 of 40 points")).toBeInTheDocument();
  });

  it("pages the reviews with the same report filter", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_list") {
        seen.push(requestOf(payload));
        return seen.length === 1
          ? { reviews: [reviewView()], nextCursor: "cursor-2" }
          : { reviews: [], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.change(screen.getByLabelText("Saved report"), { target: { value: REPORT_ID } });
    fireEvent.click(screen.getByTestId("loadReviewHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("nextReviewsPageButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("nextReviewsPageButton"));

    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]).toEqual({ reportId: REPORT_ID, cursor: "cursor-2" });
  });

  it("shows a disagreement with its status and codes raw, and acknowledges it with a null reference", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_list_disagreements") {
        return { disagreements: [disagreementView()], nextCursor: null };
      }
      if (cmd === "review_acknowledge_disagreement") {
        seen.push(requestOf(payload));
        return disagreementView({ status: "ACKNOWLEDGED" });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.click(screen.getByTestId("loadDisagreementHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("openDisagreementButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("openDisagreementButton"));

    expect(statusOf("disagreementStatusLabel")).toBe("Open");
    expect(statusOf("disagreementReasonsLabel")).toBe("Overall scores differ");
    expect(statusOf("disagreementDefectCodesLabel")).toBe("Structured evidence is missing");
    fireEvent.click(screen.getByTestId("acknowledgeMatchDisagreementButton"));

    await waitFor(() => expect(statusOf("disagreementStatusLabel")).toBe("Acknowledged"));
    expect(seen).toEqual([{ disagreementId: DISAGREEMENT_ID, issueReference: null }]);
  });

  it("pages the disagreements with the same report filter", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_list_disagreements") {
        seen.push(requestOf(payload));
        return seen.length === 1
          ? { disagreements: [disagreementView()], nextCursor: "cursor-7" }
          : { disagreements: [], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.change(screen.getByLabelText("Saved report"), { target: { value: REPORT_ID } });
    fireEvent.click(screen.getByTestId("loadDisagreementHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("nextDisagreementsPageButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("nextDisagreementsPageButton"));

    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]).toEqual({ reportId: REPORT_ID, cursor: "cursor-7" });
  });

  it("records an issue reference only when it is not blank, trimmed", async () => {
    const seen: unknown[] = [];
    serve((cmd, payload) => {
      if (cmd === "review_list_disagreements") {
        return { disagreements: [disagreementView()], nextCursor: null };
      }
      if (cmd === "review_link_disagreement") {
        seen.push(requestOf(payload));
        return disagreementView({ status: "LINKED", issueReference: "SYN-1" });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.click(screen.getByTestId("loadDisagreementHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("openDisagreementButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("openDisagreementButton"));

    expect(screen.getByTestId("linkMatchDisagreementButton")).toBeDisabled();
    expect(screen.getByTestId("reviewIssueHint")).toHaveTextContent(
      "Enter an existing issue ID before linking.",
    );
    typeInto("existingIssueReferenceInput", "  SYN-1  ");
    fireEvent.click(screen.getByTestId("linkMatchDisagreementButton"));

    await waitFor(() => expect(statusOf("disagreementStatusLabel")).toBe("Linked to an issue"));
    expect(seen).toEqual([{ disagreementId: DISAGREEMENT_ID, issueReference: "SYN-1" }]);
  });

  it("publishes the linked disagreement in the list too", async () => {
    serve((cmd) => {
      if (cmd === "review_list_disagreements") {
        return { disagreements: [disagreementView()], nextCursor: null };
      }
      if (cmd === "review_link_disagreement") {
        return disagreementView({ status: "LINKED", issueReference: "SYN-2" });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.click(screen.getByTestId("loadDisagreementHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("openDisagreementButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("openDisagreementButton"));
    typeInto("existingIssueReferenceInput", "SYN-2");
    fireEvent.click(screen.getByTestId("linkMatchDisagreementButton"));

    await waitFor(() => {
      const lists = within(screen.getByTestId("reviewListsPanel"));
      expect(lists.getByText("Linked to an issue")).toBeInTheDocument();
      expect(lists.getByText("Overall scores differ")).toBeInTheDocument();
    });
  });

  it("uses the not-found copy for a missing review and keeps the draft", async () => {
    serve((cmd) => {
      if (cmd === "review_get") {
        reject("not_found", "reviews.error.notFound");
      }
      if (cmd === "review_list") {
        return { reviews: [reviewView()], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fillIdentityFields();
    fireEvent.click(screen.getByTestId("loadReviewHistoryButton"));
    await waitFor(() => expect(screen.getByTestId("openReviewButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("openReviewButton"));

    await waitFor(() =>
      expect(statusOf("reviewStatusLabel")).toBe(
        "The saved review or report is no longer available. Your draft is kept.",
      ),
    );
    expect(screen.getByLabelText("Reviewer label")).toHaveValue(" synthetic-reviewer ");
  });
});

describe("advisory review audit fixes", () => {
  it("clamps the earned points to the new weight when the component changes", async () => {
    const report = openedReport();
    report.report.components.push({
      component: "DOMAIN",
      earnedPoints: 5,
      availablePoints: 10,
      status: "PARTIAL",
    });
    await renderReady(report);
    fireEvent.click(screen.getByTestId("addComponentDetailButton"));
    const earned = screen.getByLabelText("Earned points");
    fireEvent.change(earned, { target: { value: "30" } });
    expect(earned).toHaveValue(30);

    fireEvent.change(screen.getByLabelText("Component"), { target: { value: "DOMAIN" } });
    expect(earned).toHaveValue(10);
    expect(earned).toHaveAttribute("max", "10");
  });

  it("says when a loaded review or disagreement list is empty", async () => {
    serve((cmd) => {
      if (cmd === "review_list") {
        return { reviews: [], nextCursor: null };
      }
      if (cmd === "review_list_disagreements") {
        return { disagreements: [], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    await renderReady();
    fireEvent.click(screen.getByTestId("loadReviewHistoryButton"));
    await waitFor(() => expect(statusOf("reviewStatusLabel")).toBe("No saved reviews found."));
    fireEvent.click(screen.getByTestId("loadDisagreementHistoryButton"));
    await waitFor(() => expect(statusOf("reviewStatusLabel")).toBe("No disagreements found."));
  });

  it("explains the disabled form while the capability check is pending, and a failed check", async () => {
    mockIPC((cmd) => {
      if (cmd === "match_capabilities_get") {
        throw new Error("ipc down");
      }
      return null;
    });
    renderPage();
    expect(statusOf("reviewStatusLabel")).toBe("Checking whether advisory reviews are available…");
    await waitFor(() =>
      expect(statusOf("reviewStatusLabel")).toBe(
        "Could not check whether advisory reviews are available.",
      ),
    );
    expect(screen.getByTestId("reviewStatusLabel")).toHaveAttribute("role", "alert");
  });

  it("names each remove and open button by its row and announces failures as alerts", async () => {
    serve((cmd) => {
      if (cmd === "review_list") {
        return { reviews: [reviewView()], nextCursor: null };
      }
      if (cmd === "review_get") {
        reject("not_found", "reviews.error.notFound");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    await renderReady(openedReport());
    fireEvent.click(screen.getByTestId("addComponentDetailButton"));
    fireEvent.click(screen.getByTestId("addEvidenceConcernButton"));
    expect(
      screen.getByRole("button", { name: "Remove detail 1: Technical skills" }),
    ).toBeInTheDocument();
    const concernRemove = screen.getByRole("button", { name: "Remove detail 1: evidence concern" });
    expect(concernRemove).toBeInTheDocument();
    // WCAG 2.5.3: the accessible name contains the visible label.
    expect(concernRemove.getAttribute("aria-label")).toContain(concernRemove.textContent ?? "");

    fireEvent.click(screen.getByTestId("loadReviewHistoryButton"));
    const open = await screen.findByRole("button", { name: "Open review 1" });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    await waitFor(() =>
      expect(screen.getByTestId("reviewStatusLabel")).toHaveAttribute("role", "alert"),
    );
  });

  it("keeps the score within 0 to 100 and hints when it is not a whole number", async () => {
    await renderReady();
    const score = screen.getByLabelText("Advisory score (0–100)");
    fireEvent.change(score, { target: { value: "250" } });
    expect(score).toHaveValue(100);
    expect(score).toHaveAttribute("aria-invalid", "false");

    fireEvent.change(score, { target: { value: "" } });
    expect(score).toHaveAttribute("aria-invalid", "true");
    expect(
      screen.getByText("Enter a whole number from 0 to 100 for the advisory score."),
    ).toBeInTheDocument();
  });
});
