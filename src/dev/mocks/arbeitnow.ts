import type { ArbeitnowRowDto } from "../../ipc/bindings";
import { jobWithDescription, storeJob } from "./jobs";
import { type Args, fail, type HandlerMap, mockConfig } from "./util";

// A fictional job-board feed: twelve rows served five at a time, as scan and next page.

type Seed = [string, string, string, boolean, string, string[], string[]]; // title, company, location, remote, posted, tags, types

const FEED: Seed[] = [
  [
    "Backend Developer Go",
    "Harbor Logic",
    "Berlin",
    false,
    "2026-03-14",
    ["go", "postgresql"],
    ["full-time"],
  ],
  [
    "Frontend Engineer",
    "Northwind Apps",
    "Remote",
    true,
    "2026-03-14",
    ["react", "typescript"],
    ["full-time"],
  ],
  [
    "Data Engineer",
    "Cobalt Metrics",
    "Munich",
    false,
    "2026-03-13",
    ["python", "spark"],
    ["full-time"],
  ],
  [
    "DevOps Engineer",
    "Tidewater IT",
    "Hamburg",
    true,
    "2026-03-13",
    ["kubernetes", "terraform"],
    ["full-time"],
  ],
  [
    "QA Automation Engineer",
    "Brightpath Software",
    "Leipzig",
    false,
    "2026-03-12",
    ["java", "selenium"],
    ["full-time"],
  ],
  [
    "Softwareentwickler Java (m/w/d)",
    "Linde & Sohn Systeme",
    "Frankfurt",
    false,
    "2026-03-12",
    ["java", "spring"],
    ["full-time"],
  ],
  [
    "Mobile Developer iOS",
    "Appfelbaum GmbH",
    "Remote",
    true,
    "2026-03-11",
    ["swift", "ios"],
    ["part-time"],
  ],
  [
    "Technical Support Engineer",
    "Quartz Cloud",
    "Dresden",
    false,
    "2026-03-11",
    ["linux", "support"],
    ["full-time"],
  ],
  [
    "Product Owner Platform",
    "Vantage Retail",
    "Köln",
    false,
    "2026-03-10",
    ["agile", "product"],
    ["full-time"],
  ],
  [
    "Machine Learning Engineer",
    "Sparrow Analytics",
    "Remote",
    true,
    "2026-03-10",
    ["python", "pytorch"],
    ["full-time"],
  ],
  [
    "Werkstudent Softwareentwicklung",
    "Kiefer Technik",
    "Stuttgart",
    false,
    "2026-03-09",
    ["python", "git"],
    ["internship"],
  ],
  [
    "Security Analyst",
    "Fjord Secure",
    "Berlin",
    true,
    "2026-03-09",
    ["security", "siem"],
    ["full-time"],
  ],
];

function rowId(index: number): string {
  return `an-row-${String(index + 1).padStart(3, "0")}`;
}

function descriptionOf(seed: Seed): string {
  const [title, company, location, , , tags] = seed;
  return [
    `${company} is hiring: ${title} in ${location}.`,
    "",
    "Your tasks",
    `- Build and maintain software together with a small, friendly team.`,
    `- Work with ${tags.join(" and ")} in everyday projects.`,
    "- Take part in code reviews and planning.",
    "",
    "Your profile",
    "- Relevant experience and the will to keep learning.",
    "- Good communication in German or English.",
  ].join("\n");
}

function toRow(seed: Seed, index: number): ArbeitnowRowDto {
  const [title, company, location, remote, postedAt, tags, jobTypes] = seed;
  return {
    rowId: rowId(index),
    title,
    company,
    location,
    remote,
    postedAt,
    excerpt: descriptionOf(seed).slice(0, 280),
    tags,
    jobTypes,
    importStatus: "none",
  };
}

let offset = 0;
let matching: number[] = [];
const imported = new Map<string, string>();

function page(count: number) {
  const slice = matching.slice(offset, offset + count);
  offset += slice.length;
  return {
    rows: slice.map((index) => toRow(FEED[index], index)),
    hasNextPage: offset < matching.length,
    consistencyNotice: "Results come from a synthetic feed and may repeat across pages.",
  };
}

export const arbeitnowHandlers: HandlerMap = {
  arbeitnow_capabilities_get: () => ({ arbeitnow: true }),
  arbeitnow_scan: ({ request }: Args) => {
    const req = request as {
      query: string | null;
      location: string | null;
      remoteOnly: boolean;
      tags: string;
      jobTypes: string;
    };
    const wantedTags = req.tags
      .toLowerCase()
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean);
    const wantedTypes = req.jobTypes
      .toLowerCase()
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean);
    matching = FEED.flatMap((seed, index) => {
      const [title, company, location, remote, , tags, types] = seed;
      const text = `${title} ${company}`.toLowerCase();
      const ok =
        (!req.query || text.includes(req.query.toLowerCase())) &&
        (!req.location || location.toLowerCase().includes(req.location.toLowerCase())) &&
        (!req.remoteOnly || remote) &&
        wantedTags.every((tag) => tags.includes(tag)) &&
        wantedTypes.every((type) => types.includes(type));
      return ok ? [index] : [];
    });
    if (mockConfig.empty) {
      matching = [];
    }
    offset = 0;
    return page(5);
  },
  arbeitnow_next_page: () => page(5),
  arbeitnow_import_row: ({ rowId: id }: Args) => {
    const index = FEED.findIndex((_, i) => rowId(i) === id);
    if (index < 0) {
      return fail("not_found", "arbeitnow.error.notFound");
    }
    const known = imported.get(String(id));
    if (known) {
      return { rowId: String(id), status: "reusedExistingJob", jobId: known };
    }
    const seed = FEED[index];
    const description = descriptionOf(seed);
    const existing = jobWithDescription(description);
    const jobId =
      existing?.job.id ??
      storeJob(
        {
          title: seed[0],
          company: seed[1],
          location: seed[2],
          description,
          sourceLabel: "Arbeitnow",
          employmentType: seed[6][0] ?? "",
          seniority: "",
          experienceRequirement: "",
          postedAt: seed[4],
          sourceMethod: "arbeitnow",
        },
        seed[5],
      ).job.id;
    imported.set(String(id), jobId);
    return { rowId: String(id), status: "createdJob", jobId };
  },
};
