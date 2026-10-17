import type { DisagreementViewDto, ReviewViewDto } from "../../ipc/bindings";
import { reportIds, reportView } from "./matching";
import { type Args, fail, type HandlerMap, nowIso, pageOf, requestOf, seed, uuid } from "./util";

// Advisory reviews of the synthetic match reports, and the disagreements they raise.

const POLICY = "divergence-v1";
const reviews: Map<string, ReviewViewDto> = new Map();
const disagreements: Map<string, DisagreementViewDto> = new Map();

type Row = Record<string, unknown>;

function reasonsFor(
  report: NonNullable<ReturnType<typeof reportView>>["report"],
  review: ReviewViewDto,
): string[] {
  const reasons: string[] = [];
  if (Math.abs(review.overallScore - report.overallScore) >= 10) {
    reasons.push("score_delta_exceeds_threshold");
  }
  if (review.outcome !== report.outcome) {
    reasons.push("outcome_differs");
  }
  if (review.blockerMismatch !== report.blockerMismatch) {
    reasons.push("blocker_mismatch_differs");
  }
  return reasons;
}

function raise(review: ReviewViewDto, reasons: string[], status = "open"): DisagreementViewDto {
  const defects = review.evidence.filter((e) => e.outcomeChangingDefect).map((e) => e.fact);
  const disagreement: DisagreementViewDto = {
    id: uuid(5, disagreements.size + 1),
    reportId: review.reportId,
    reviewId: review.id,
    policyVersion: POLICY,
    status,
    createdAt: review.createdAt,
    updatedAt: review.createdAt,
    issueReference: null,
    reasons,
    evidenceDefectCodes: defects,
    reasonsTotalCount: reasons.length,
    reasonsTruncated: false,
    defectsTotalCount: defects.length,
    defectsTruncated: false,
  };
  disagreements.set(disagreement.id, disagreement);
  return disagreement;
}

function seedReview(
  n: number,
  reportIndex: number,
  reviewer: string,
  delta: number,
  outcome: string | null,
  summary: string,
): ReviewViewDto | null {
  const reportId = reportIds()[reportIndex];
  const view = reportId ? reportView(reportId) : undefined;
  if (!view) {
    return null;
  }
  const { report } = view;
  const review: ReviewViewDto = {
    id: uuid(4, n),
    reportId: report.id,
    reviewer,
    model: "example-reviewer-1",
    reviewVersion: "review-v1",
    outcome: outcome ?? report.outcome,
    summary,
    createdAt: `2026-03-1${n + 1}T14:00:00Z`,
    overallScore: Math.max(0, Math.min(100, report.overallScore + delta)),
    blockerMismatch: report.blockerMismatch,
    components: report.components.map((c) => ({ ...c })),
    evidence: report.evidence
      .slice(0, 2)
      .map((e) => ({ ...e, outcomeChangingDefect: delta !== 0 && e.status === "UNKNOWN" })),
    componentsTotalCount: report.components.length,
    componentsTruncated: false,
    evidenceTotalCount: Math.min(2, report.evidence.length),
    evidenceTruncated: false,
  };
  reviews.set(review.id, review);
  return review;
}

for (const [n, args] of seed([
  [1, 0, "reviewer-a", 0, null, "review_consistent"],
  [2, 4, "reviewer-b", 18, "PARTIAL_MATCH", "score_adjustment"],
  [3, 2, "reviewer-a", -12, null, "evidence_defect_identified"],
] as const).entries()) {
  const review = seedReview(args[0], args[1], args[2], args[3], args[4], args[5]);
  const report = review ? reportView(review.reportId)?.report : undefined;
  if (review && report) {
    const reasons = reasonsFor(report, review);
    if (reasons.length > 0) {
      const d = raise(review, reasons, n === 2 ? "acknowledged" : "open");
      if (n === 2) {
        disagreements.set(d.id, { ...d, issueReference: "EXAMPLE-142", updatedAt: nowIso() });
      }
    }
  }
}

function requireDisagreement(id: unknown): DisagreementViewDto {
  return disagreements.get(String(id)) ?? fail("not_found", "reviews.error.notFound");
}

function updateDisagreement(
  id: unknown,
  status: string,
  issueReference: unknown,
): DisagreementViewDto {
  const current = requireDisagreement(id);
  const next: DisagreementViewDto = {
    ...current,
    status,
    issueReference: typeof issueReference === "string" ? issueReference : current.issueReference,
    updatedAt: nowIso(),
  };
  disagreements.set(next.id, next);
  return structuredClone(next);
}

export const reviewHandlers: HandlerMap = {
  review_set_draft_dirty: () => null,
  review_submit: (args: Args) => {
    const request = requestOf(args);
    const reportId = String(request.reportId ?? "");
    const view = reportView(reportId);
    if (!view) {
      return fail("not_found", "reviews.error.notFound");
    }
    if (typeof request.overallScore !== "number" || String(request.reviewer ?? "").trim() === "") {
      fail("validation_error", "reviews.error.validation");
    }
    const components = (request.components as Row[] | undefined) ?? [];
    const evidence = (request.evidence as Row[] | undefined) ?? [];
    const review: ReviewViewDto = {
      id: uuid(4, reviews.size + 1),
      reportId,
      reviewer: String(request.reviewer),
      model: String(request.model ?? ""),
      reviewVersion: String(request.reviewVersion ?? ""),
      outcome: String(request.outcome ?? view.report.outcome),
      summary: String(request.summary ?? ""),
      createdAt: nowIso(),
      overallScore: request.overallScore,
      blockerMismatch: Boolean(request.blockerMismatch),
      components: components.map((c) => ({
        component: String(c.component),
        earnedPoints: Number(c.earnedPoints),
        availablePoints: Number(c.availablePoints),
        status: String(c.status),
      })),
      evidence: evidence.map((e) => ({
        component: String(e.component),
        status: String(e.status),
        sourceType: "reviewer",
        fact: String(e.fact),
        outcomeChangingDefect: Boolean(e.outcomeChangingDefect),
      })),
      componentsTotalCount: components.length,
      componentsTruncated: false,
      evidenceTotalCount: evidence.length,
      evidenceTruncated: false,
    };
    reviews.set(review.id, review);
    const reasons = reasonsFor(view.report, review);
    return {
      review: structuredClone(review),
      disagreement: reasons.length > 0 ? structuredClone(raise(review, reasons)) : null,
    };
  },
  review_get: (args: Args) => {
    const review = reviews.get(String(requestOf(args).id));
    return review ? structuredClone(review) : fail("not_found", "reviews.error.notFound");
  },
  review_list: (args: Args) => {
    const request = requestOf(args);
    const rows = [...reviews.values()]
      .reverse()
      .filter((r) => typeof request.reportId !== "string" || r.reportId === request.reportId);
    const { items, nextCursor } = pageOf(rows, request.cursor, 10);
    return { reviews: structuredClone(items), nextCursor };
  },
  review_list_disagreements: (args: Args) => {
    const request = requestOf(args);
    const rows = [...disagreements.values()]
      .reverse()
      .filter((d) => typeof request.reportId !== "string" || d.reportId === request.reportId);
    const { items, nextCursor } = pageOf(rows, request.cursor, 10);
    return { disagreements: structuredClone(items), nextCursor };
  },
  review_acknowledge_disagreement: (args: Args) => {
    const request = requestOf(args);
    return updateDisagreement(request.disagreementId, "acknowledged", request.issueReference);
  },
  review_link_disagreement: (args: Args) => {
    const request = requestOf(args);
    return updateDisagreement(request.disagreementId, "linked", request.issueReference);
  },
};
