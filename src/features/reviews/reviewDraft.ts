/**
 * The advisory review draft. React holds the draft; Rust validates
 * the whole draft on save. The fields and the enum values are the wire values, and the
 * limits here are only the UX pre-checks.
 */

export const OUTCOMES = [
  "STRONG_MATCH",
  "PARTIAL_MATCH",
  "WEAK_MATCH",
  "INSUFFICIENT_EVIDENCE",
] as const;

export const SUMMARIES = [
  "review_consistent",
  "score_adjustment",
  "outcome_adjustment",
  "evidence_defect_identified",
] as const;

export const STATUSES = ["MATCH", "PARTIAL", "MISMATCH", "UNKNOWN"] as const;

export const COMPONENTS = [
  "TECHNICAL",
  "EXPERIENCE_SENIORITY",
  "DOMAIN",
  "DELIVERY",
  "HARD_REQUIREMENTS",
] as const;

export const FACTS = [
  "structured_evidence_missing",
  "structured_evidence_incorrect",
  "requirement_provenance_missing",
  "outcome_calibration_issue",
] as const;

/** The most component details a review holds. */
export const MAX_DETAIL_ROWS = 100;
/** The most evidence concerns a review holds. */
export const MAX_CONCERN_ROWS = 1000;
/** The longest identifier the form accepts. */
export const MAX_ID_LENGTH = 36;
/** The longest label the form accepts. */
export const MAX_LABEL_LENGTH = 80;
/** The longest issue reference the form accepts. */
export const MAX_ISSUE_LENGTH = 256;
/** The highest advisory score: the presenter's spin box and its preview both stop at 100. */
export const MAX_SCORE = 100;

/** One component detail: its points are bounded by the component's weight in the report. */
export type ComponentDetail = {
  id: number;
  component: string;
  status: string;
  earnedPoints: number;
  availablePoints: number;
};

/** One evidence concern. The source type is fixed by Rust, so it is not part of the draft. */
export type EvidenceConcern = {
  id: number;
  component: string;
  status: string;
  fact: string;
  outcomeChangingDefect: boolean;
};

/** The whole draft. Score is the text of the spin box, converted only when it is saved. */
export type ReviewDraft = {
  reportId: string;
  reviewer: string;
  model: string;
  reviewVersion: string;
  score: string;
  outcome: string;
  summary: string;
  blockerMismatch: boolean;
  details: ComponentDetail[];
  concerns: EvidenceConcern[];
};

/** The request `review_submit` reads (its request keys). */
export type ReviewSubmitRequest = {
  reportId: string;
  reviewer: string;
  model: string;
  reviewVersion: string;
  overallScore: number | null;
  outcome: string;
  summary: string;
  blockerMismatch: boolean;
  components: {
    component: string;
    earnedPoints: number;
    availablePoints: number;
    status: string;
  }[];
  evidence: { component: string; status: string; fact: string; outcomeChangingDefect: boolean }[];
};

/** The draft a fresh screen and a discard start from (the draft defaults). */
export function emptyDraft(): ReviewDraft {
  return {
    reportId: "",
    reviewer: "",
    model: "",
    reviewVersion: "",
    score: "0",
    outcome: "INSUFFICIENT_EVIDENCE",
    summary: "review_consistent",
    blockerMismatch: false,
    details: [],
    concerns: [],
  };
}

/** The four identity fields must be non-empty after trimming before a save is offered. */
export function identityReady(draft: ReviewDraft): boolean {
  return [draft.reportId, draft.reviewer, draft.model, draft.reviewVersion].every(
    (value) => value.trim() !== "",
  );
}

/**
 * A whole number from field text, or null when the text is not one. The accepted text is ASCII digits after
 * trimming, with no sign, fraction, exponent or radix prefix (Number() would read "1e2" and "0x10").
 */
export function wholeNumber(text: string): number | null {
  const trimmed = text.trim();
  return /^[0-9]+$/.test(trimmed) ? Number(trimmed) : null;
}

/** The score as an integer from 0 to MAX_SCORE, or null when it is not one (Rust then refuses it). */
export function scoreValue(text: string): number | null {
  const value = wholeNumber(text);
  return value !== null && value <= MAX_SCORE ? value : null;
}

/**
 * The draft without the React row ids. Two drafts with the same signature are the same review, so
 * the dirty flag compares signatures and not object identity.
 */
export function signature(draft: ReviewDraft): string {
  return JSON.stringify({
    ...draft,
    details: draft.details.map((row) => ({
      component: row.component,
      status: row.status,
      earnedPoints: row.earnedPoints,
      availablePoints: row.availablePoints,
    })),
    concerns: draft.concerns.map((row) => ({
      component: row.component,
      status: row.status,
      fact: row.fact,
      outcomeChangingDefect: row.outcomeChangingDefect,
    })),
  });
}

/** The save request for a draft: identifiers and labels trimmed, rows in their order. */
export function submitRequest(draft: ReviewDraft): ReviewSubmitRequest {
  return {
    reportId: draft.reportId.trim(),
    reviewer: draft.reviewer.trim(),
    model: draft.model.trim(),
    reviewVersion: draft.reviewVersion.trim(),
    overallScore: scoreValue(draft.score),
    outcome: draft.outcome,
    summary: draft.summary,
    blockerMismatch: draft.blockerMismatch,
    components: draft.details.map((row) => ({
      component: row.component,
      earnedPoints: row.earnedPoints,
      availablePoints: row.availablePoints,
      status: row.status,
    })),
    evidence: draft.concerns.map((row) => ({
      component: row.component,
      status: row.status,
      fact: row.fact,
      outcomeChangingDefect: row.outcomeChangingDefect,
    })),
  };
}

/** The optional report filter of a list: a blank report ID means no filter. */
export function reportFilter(reportId: string): string | null {
  const trimmed = reportId.trim();
  return trimmed === "" ? null : trimmed;
}
