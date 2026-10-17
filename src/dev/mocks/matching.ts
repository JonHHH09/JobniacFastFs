import type {
  MatchComponentDto,
  MatchEvidenceDto,
  MatchReportDto,
  MatchReportViewDto,
} from "../../ipc/bindings";
import { jobDetail, jobIds } from "./jobs";
import { profileGraph, profileIds } from "./profiles";
import {
  type Args,
  fail,
  freshId,
  type HandlerMap,
  nowIso,
  pageOf,
  requestOf,
  seed,
  uuid,
} from "./util";

// Deterministic reports computed from the skill overlap of the synthetic profile and job.

const ALGORITHM = "deterministic-v3";
const reports: Map<string, MatchReportViewDto> = new Map();
const staleReports = new Set<string>();

function ratio(profileId: string, jobId: string): { overlap: number; shared: string[] } {
  const profile = profileGraph(profileId);
  const job = jobDetail(jobId);
  if (!profile || !job) {
    return fail("not_found", "matching.error.notFound");
  }
  const have = new Set(profile.skills.map((s) => s.normalizedSkill));
  const wanted = job.skills.map((s) => s.toLowerCase());
  const shared = job.skills.filter((s) => have.has(s.toLowerCase()));
  return { overlap: wanted.length === 0 ? 0 : shared.length / wanted.length, shared };
}

function statusOf(earned: number, available: number): string {
  const part = earned / available;
  if (part >= 0.75) return "MATCH";
  if (part >= 0.4) return "PARTIAL";
  return "MISMATCH";
}

function buildReport(
  id: string,
  profileId: string,
  jobId: string,
  createdAt: string,
): MatchReportViewDto {
  const profile = profileGraph(profileId);
  const job = jobDetail(jobId);
  if (!profile || !job) {
    return fail("not_found", "matching.error.notFound");
  }
  const { overlap, shared } = ratio(profileId, jobId);
  const senior = /senior|lead/i.test(job.job.seniority) ? 1 : 0;
  const years = profile.experiences.length;
  const points: [string, number, number][] = [
    ["TECHNICAL", Math.round(40 * overlap), 40],
    ["EXPERIENCE_SENIORITY", Math.min(25, 10 + years * 6 - senior * 4), 25],
    ["DOMAIN", Math.round(15 * Math.min(1, overlap + 0.25)), 15],
    ["DELIVERY", 7 + (years > 1 ? 2 : 0), 10],
    ["HARD_REQUIREMENTS", overlap > 0.2 ? 10 : 2, 10],
  ];
  const components: MatchComponentDto[] = points.map(
    ([component, earnedPoints, availablePoints]) => ({
      component,
      earnedPoints,
      availablePoints,
      status: statusOf(earnedPoints, availablePoints),
    }),
  );
  const overallScore = components.reduce((sum, c) => sum + c.earnedPoints, 0);
  const evidence: MatchEvidenceDto[] = [
    ...shared.map((skill) => ({
      component: "TECHNICAL",
      status: "MATCH",
      sourceType: "profile_skill",
      fact: `Profile lists ${skill}, which the posting asks for.`,
      outcomeChangingDefect: false,
    })),
    ...job.skills
      .filter((skill) => !shared.includes(skill))
      .map((skill) => ({
        component: "TECHNICAL",
        status: "UNKNOWN",
        sourceType: "job_requirement",
        fact: `No evidence of ${skill} in the profile.`,
        outcomeChangingDefect: false,
      })),
    {
      component: "EXPERIENCE_SENIORITY",
      status: components[1].status,
      sourceType: "profile_experience",
      fact: `${years} documented position${years === 1 ? "" : "s"} against a ${job.job.seniority} posting.`,
      outcomeChangingDefect: false,
    },
  ];
  const blockerMismatch = overlap < 0.2;
  const outcome =
    blockerMismatch || overallScore < 40
      ? "WEAK_MATCH"
      : overallScore >= 75
        ? "STRONG_MATCH"
        : "PARTIAL_MATCH";
  const report: MatchReportDto = {
    id,
    profileId,
    jobId,
    profileRevision: profile.profile.updatedAt,
    jobRevision: createdAt,
    algorithmVersion: ALGORITHM,
    outcome,
    createdAt,
    overallScore,
    confidence: Math.round(60 + 30 * Math.min(1, job.skills.length / 4)),
    blockerMismatch,
    components,
    evidence,
  };
  return {
    stale: false,
    report,
    componentsTruncated: false,
    componentsTotalCount: components.length,
    evidenceTruncated: false,
    evidenceTotalCount: evidence.length,
  };
}

/** The ids of the stored reports, newest first, for the review fixtures. */
export function reportIds(): string[] {
  return [...reports.keys()];
}

/** One stored report, or undefined. */
export function reportView(id: string): MatchReportViewDto | undefined {
  return reports.get(id);
}

// (profile index, job index) pairs, one to a few reports each; the last one is stale.
const SEEDED: [number, number][] = [
  [0, 0],
  [1, 1],
  [2, 2],
  [3, 3],
  [0, 9],
  [4, 4],
];

if (profileIds().length > 0 && jobIds().length > 0) {
  for (const [n, [p, j]] of seed(SEEDED).entries()) {
    const id = uuid(3, n + 1);
    const created = `2026-03-${String(10 + n).padStart(2, "0")}T10:15:00Z`;
    reports.set(id, buildReport(id, profileIds()[p], jobIds()[j], created));
  }
  const keys = [...reports.keys()];
  const last = keys[keys.length - 1];
  if (last) {
    staleReports.add(last);
  }
}

function viewOf(id: string): MatchReportViewDto {
  const stored = reports.get(id);
  if (!stored) {
    return fail("not_found", "matching.error.notFound");
  }
  return { ...structuredClone(stored), stale: staleReports.has(id) };
}

function analyze(profileId: string, jobId: string): MatchReportViewDto {
  const id = freshId();
  reports.set(id, buildReport(id, profileId, jobId, nowIso()));
  return viewOf(id);
}

export const matchingHandlers: HandlerMap = {
  match_capabilities_get: () => ({ matching: true, reviews: true }),
  match_analyze_pair: (args: Args) => {
    const request = requestOf(args);
    return analyze(String(request.profileId), String(request.jobId));
  },
  match_analyze_page: (args: Args) => {
    const request = requestOf(args);
    const profileId = String(request.profileId);
    if (!profileGraph(profileId)) {
      fail("not_found", "matching.error.notFound");
    }
    const { items, nextCursor } = pageOf(jobIds(), request.cursor, 5);
    return { succeeded: items.map((jobId) => analyze(profileId, jobId)), failed: [], nextCursor };
  },
  match_list_reports: (args: Args) => {
    const request = requestOf(args);
    const rows = [...reports.keys()]
      .reverse()
      .map(viewOf)
      .filter(
        (view) =>
          (typeof request.profileId !== "string" || view.report.profileId === request.profileId) &&
          (typeof request.jobId !== "string" || view.report.jobId === request.jobId),
      );
    const { items, nextCursor } = pageOf(rows, request.cursor, 10);
    return { reports: items, nextCursor };
  },
  match_get_report: (args: Args) => viewOf(String(requestOf(args).id)),
};
