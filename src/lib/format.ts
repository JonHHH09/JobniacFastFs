import { formatMessage, isMessageId, type MessageId } from "../i18n/messages";

/**
 * Every enum the screens display, with the values that have a label in the catalogue
 * (`enum.<kind>.<value>` in `src/i18n/messages.ts`). Wire values sent to Rust never change; this
 * only decides the text on screen. A value that is not listed falls back to a sentence-cased copy
 * of the raw value, so a new value from Rust is readable before it gets copy.
 *
 * - matchComponent: component of a match report or review (`MatchComponentDto.component`).
 * - matchStatus: status of one component (`MatchComponentDto.status`, `MatchEvidenceDto.status`).
 * - matchOutcome: outcome band of a report or review (`outcome`).
 * - evidenceSource: `MatchEvidenceDto.sourceType`.
 * - reviewSummary: advisory summary code of a review (`ReviewViewDto.summary`).
 * - evidenceDefect: defect codes of a disagreement (`evidenceDefectCodes`) and review evidence facts.
 * - disagreementStatus: workflow status of a disagreement (`DisagreementViewDto.status`).
 * - disagreementReason: reasons of a disagreement (`DisagreementViewDto.reasons`).
 * - jobType: Arbeitnow job types (free text from the service; matched ignoring case and
 *   punctuation, so "Full-Time" and "full_time" are the same key).
 * - healthStatus: `HealthStatusDto`.
 * - errorCategory: `ErrorCategoryDto` of the health check.
 * - cleanupStatus: `CleanupStatusDto` of the generated resume cleanup.
 * - captureStatus: `CaptureStatusDto` of a job capture.
 * - importStatus: `ArbeitnowImportStatusDto` and the row status `none`.
 * - ingestStatus: `IngestStatus` of the profile-from-PDF flow.
 * - matchedOn: `MatchedOn` reasons of a duplicate-profile candidate.
 * - profileField: `matchedFields` of a profile search result.
 * - generationKind: the kinds of the generation form (`GenerationResumeRequest.kind` and standalone).
 * - generationVariant: `GenerationVariantDto`.
 * - labelPrefix: `LabelPrefixDto` of a generated output.
 * - sourceType: `SourceType` of a profile source link.
 */
export const ENUM_VALUES = {
  matchComponent: ["TECHNICAL", "EXPERIENCE_SENIORITY", "DOMAIN", "DELIVERY", "HARD_REQUIREMENTS"],
  matchStatus: ["MATCH", "PARTIAL", "MISMATCH", "UNKNOWN"],
  matchOutcome: ["STRONG_MATCH", "PARTIAL_MATCH", "WEAK_MATCH", "INSUFFICIENT_EVIDENCE"],
  evidenceSource: ["normalized_skill", "advisory_defect"],
  reviewSummary: [
    "review_consistent",
    "score_adjustment",
    "outcome_adjustment",
    "evidence_defect_identified",
  ],
  evidenceDefect: [
    "structured_evidence_missing",
    "structured_evidence_incorrect",
    "requirement_provenance_missing",
    "outcome_calibration_issue",
  ],
  disagreementStatus: ["OPEN", "PENDING_ESCALATION", "ACKNOWLEDGED", "LINKED"],
  disagreementReason: [
    "OVERALL_DELTA",
    "OUTCOME_MISMATCH",
    "BLOCKER_MISMATCH",
    "COMPONENT_DELTA",
    "EVIDENCE_DEFECT",
  ],
  jobType: [
    "full_time",
    "part_time",
    "contract",
    "freelance",
    "internship",
    "working_student",
    "apprenticeship",
    "temporary",
    "volunteer",
    "professional_experienced",
  ],
  healthStatus: ["UP", "DOWN"],
  errorCategory: [
    "NONE",
    "CONNECTION_UNAVAILABLE",
    "AUTHENTICATION_FAILED",
    "QUERY_FAILED",
    "TIMEOUT",
    "UNKNOWN",
  ],
  cleanupStatus: ["UNKNOWN", "DEGRADED", "HEALTHY"],
  captureStatus: ["created_job", "reused_existing_job"],
  importStatus: ["createdJob", "reusedExistingJob", "none"],
  ingestStatus: [
    "createdProfile",
    "updatedProfile",
    "reusedExistingSource",
    "duplicateProfileCandidate",
    "ambiguousProfileCandidates",
  ],
  matchedOn: ["email", "github", "linkedin", "otherLink", "profileId"],
  profileField: ["fullName", "email", "summary", "skills", "experience", "education", "projects"],
  generationKind: [
    "standalone",
    "masterResume",
    "canadianResume",
    "canadianFrenchResume",
    "germanyResume",
    "germanCoverLetter",
  ],
  generationVariant: ["single", "germanTailored"],
  labelPrefix: ["PDF", "EN", "DE"],
  sourceType: ["resumePdf"],
} as const satisfies Record<string, readonly string[]>;

export type EnumKind = keyof typeof ENUM_VALUES;

/** The catalogue id of one enum value. */
export function enumMessageId(kind: EnumKind, value: string): string {
  return `enum.${kind}.${value}`;
}

/** "SOME_VALUE", "someValue" and "some-value" read "Some value". */
export function sentenceCase(raw: string): string {
  const words = raw
    .trim()
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_\-\s]+/g, " ")
    .trim()
    .toLowerCase();
  return words === "" ? "" : words.charAt(0).toUpperCase() + words.slice(1);
}

function lookup(kind: EnumKind, value: string): MessageId | null {
  const id = enumMessageId(kind, value);
  return isMessageId(id) ? id : null;
}

/**
 * The display label of an enum value of the given kind. Tries the exact value, then (for kinds
 * whose wire form is upper case) the upper-cased value, and for job types a normalised key. Unknown
 * values return a sentence-cased version of the raw value; an empty value returns "".
 */
export function humanizeEnum(kind: EnumKind, value: string | null | undefined): string {
  if (value === null || value === undefined || value.trim() === "") {
    return "";
  }
  const raw = value.trim();
  const candidates =
    kind === "jobType"
      ? [
          raw
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, "_")
            .replace(/^_+|_+$/g, ""),
        ]
      : [raw, raw.toUpperCase(), raw.toLowerCase()];
  for (const candidate of candidates) {
    const id = lookup(kind, candidate);
    if (id !== null) {
      return formatMessage(id);
    }
  }
  return sentenceCase(raw);
}

function toDate(value: string | number | Date | null | undefined): Date | null {
  if (value === null || value === undefined || value === "") {
    return null;
  }
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

type InstantOptions = { locale?: string; timeZone?: string };

/**
 * An instant as the user's locale shows a date and time ("Mar 15, 2026, 10:15 AM"). Text that is
 * not a date is returned as it is, so no information is lost; null and undefined give "".
 */
export function formatInstant(
  value: string | number | Date | null | undefined,
  options: InstantOptions = {},
): string {
  if (value === null || value === undefined || value === "") {
    return "";
  }
  const date = toDate(value);
  if (date === null) {
    return String(value);
  }
  return new Intl.DateTimeFormat(options.locale, {
    dateStyle: "medium",
    timeStyle: "short",
    ...(options.timeZone ? { timeZone: options.timeZone } : {}),
  }).format(date);
}

const RELATIVE_STEPS: readonly [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];

/**
 * A secondary hint such as "2 days ago" or "in 3 hours" relative to `now`. Returns "" for a value
 * that is not a date. Under a minute it reads "now".
 */
export function formatRelative(
  value: string | number | Date | null | undefined,
  now: number | Date = Date.now(),
  locale?: string,
): string {
  const date = toDate(value);
  if (date === null) {
    return "";
  }
  const formatter = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  const seconds = Math.round((date.getTime() - (now instanceof Date ? now.getTime() : now)) / 1000);
  const abs = Math.abs(seconds);
  for (const [unit, size] of RELATIVE_STEPS) {
    if (abs >= size) {
      return formatter.format(Math.trunc(seconds / size), unit);
    }
  }
  return formatter.format(0, "second");
}

/** A byte count as "812 B", "12 KB" or "1.4 MB" (binary units, one decimal under 10). */
export function formatBytes(bytes: number | string | null | undefined, locale?: string): string {
  const n = typeof bytes === "string" ? Number(bytes) : bytes;
  if (n === null || n === undefined || !Number.isFinite(n) || n < 0) {
    return "";
  }
  const units = ["B", "KB", "MB", "GB"];
  let size = n;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  const text = new Intl.NumberFormat(locale, {
    maximumFractionDigits: unit > 0 && size < 10 ? 1 : 0,
  }).format(size);
  return `${text} ${units[unit]}`;
}
