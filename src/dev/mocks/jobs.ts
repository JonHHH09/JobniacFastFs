import type { JobDetailDto, JobFieldsDto, JobSearchDto, JobSummaryDto } from "../../ipc/bindings";
import {
  type Args,
  fail,
  freshId,
  type HandlerMap,
  nextRevision,
  nowIso,
  pageOf,
  requestOf,
  seed,
  tokensOf,
  uuid,
} from "./util";

// Synthetic postings: fictional companies, generic duties, no real employer or person.

type Lang = "en" | "de";

type Spec = {
  title: string;
  company: string;
  location: string;
  lang: Lang;
  type: string;
  seniority: string;
  experience: string;
  skills: string[];
  blurb: string;
  posted: string;
  source: string;
};

const SPECS: Spec[] = [
  {
    title: "Senior Backend Engineer (Rust)",
    company: "Lumenwork GmbH",
    location: "Hamburg",
    lang: "en",
    type: "Full-time",
    seniority: "Senior",
    experience: "5+ years",
    skills: ["Rust", "PostgreSQL", "gRPC", "Kubernetes"],
    blurb:
      "Lumenwork builds an event ingestion platform that processes terabytes of telemetry a day. You will own services on the hot path and help us move more of them to Rust.",
    posted: "2026-03-02",
    source: "Company site",
  },
  {
    title: "Frontend Developer React",
    company: "Pixelharbor AG",
    location: "Remote (EU)",
    lang: "en",
    type: "Full-time",
    seniority: "Mid-level",
    experience: "3+ years",
    skills: ["TypeScript", "React", "CSS", "Accessibility"],
    blurb:
      "Join the team behind our customer portal, used by more than 100,000 people every month. We care about accessible, fast interfaces and a shared design system.",
    posted: "2026-03-05",
    source: "Job board",
  },
  {
    title: "Data Engineer (m/w/d)",
    company: "Quillstone Analytics",
    location: "München",
    lang: "de",
    type: "Vollzeit",
    seniority: "Mittel",
    experience: "3 Jahre",
    skills: ["Python", "SQL", "dbt", "Airflow"],
    blurb:
      "Quillstone Analytics hilft Handelsunternehmen, aus Daten Entscheidungen zu machen. Für unser Data-Platform-Team suchen wir Verstärkung, die verlässliche Pipelines baut.",
    posted: "2026-03-06",
    source: "Karriereseite",
  },
  {
    title: "Site Reliability Engineer",
    company: "Moorlight Cloud Services",
    location: "Stuttgart",
    lang: "en",
    type: "Full-time",
    seniority: "Senior",
    experience: "5+ years",
    skills: ["Terraform", "Linux", "Prometheus", "GitHub Actions"],
    blurb:
      "Our platform team runs 40 production clusters for customers across Europe. You will improve reliability, automate toil and take part in a humane on-call rotation.",
    posted: "2026-03-07",
    source: "Company site",
  },
  {
    title: "Embedded Software Engineer",
    company: "Ironvale Automation",
    location: "Tampere, Finland",
    lang: "en",
    type: "Full-time",
    seniority: "Mid-level",
    experience: "4+ years",
    skills: ["C", "C++", "RTOS", "CAN bus"],
    blurb:
      "Ironvale develops motor controllers for industrial machinery. You will write firmware, extend our hardware-in-the-loop test rig and work closely with electrical engineers.",
    posted: "2026-03-08",
    source: "Job board",
  },
  {
    title: "Machine Learning Engineer Ranking",
    company: "Stagecoach Media",
    location: "Amsterdam",
    lang: "en",
    type: "Full-time",
    seniority: "Senior",
    experience: "5+ years",
    skills: ["Python", "PyTorch", "MLflow", "SQL"],
    blurb:
      "Help millions of viewers find something to watch. The ranking team trains and evaluates models, runs controlled experiments and ships improvements every week.",
    posted: "2026-03-09",
    source: "Company site",
  },
  {
    title: "Testautomatisierer / QA Engineer (m/w/d)",
    company: "Edelweiss Software",
    location: "Graz, Österreich",
    lang: "de",
    type: "Vollzeit",
    seniority: "Mittel",
    experience: "2 Jahre",
    skills: ["Java", "Playwright", "Jenkins"],
    blurb:
      "Bei Edelweiss Software entwickeln wir Branchensoftware für den Mittelstand. Du sorgst dafür, dass jede Auslieferung getestet, nachvollziehbar und stressfrei ist.",
    posted: "2026-03-10",
    source: "Karriereseite",
  },
  {
    title: "Full-stack Developer Python and React",
    company: "Fieldnote Ltd",
    location: "Bristol, UK (hybrid)",
    lang: "en",
    type: "Full-time",
    seniority: "Mid-level",
    experience: "3+ years",
    skills: ["Python", "Django", "React", "PostgreSQL"],
    blurb:
      "Fieldnote makes note-taking software for researchers who work offline. You will build features end to end, from database schema to the last pixel.",
    posted: "2026-03-11",
    source: "Job board",
  },
  {
    title: "Softwareentwickler Backend Java (m/w/d)",
    company: "Nordlicht Versicherungen",
    location: "Hannover",
    lang: "de",
    type: "Vollzeit",
    seniority: "Mittel",
    experience: "3 Jahre",
    skills: ["Java", "Spring Boot", "PostgreSQL", "REST"],
    blurb:
      "Die Nordlicht Versicherungen modernisieren ihre Vertragsverwaltung. Du entwickelst Services, die täglich Tausende Vorgänge zuverlässig verarbeiten.",
    posted: "2026-03-12",
    source: "Karriereseite",
  },
  {
    title: "DevOps Engineer Kubernetes",
    company: "Cloudmeadow GmbH",
    location: "Berlin",
    lang: "de",
    type: "Vollzeit",
    seniority: "Senior",
    experience: "5 Jahre",
    skills: ["Kubernetes", "Terraform", "Helm", "Go"],
    blurb:
      "Cloudmeadow betreibt eine Plattform für Software-Teams im Mittelstand. Du entwickelst unsere Infrastruktur weiter und befähigst Entwicklerteams zu schnellen Releases.",
    posted: "2026-03-13",
    source: "Job board",
  },
  {
    title: "Product Designer",
    company: "Pixelharbor AG",
    location: "Remote (EU)",
    lang: "en",
    type: "Full-time",
    seniority: "Mid-level",
    experience: "3+ years",
    skills: ["Figma", "Design systems", "User research"],
    blurb:
      "You will shape the next generation of our customer portal together with product managers and engineers, from first sketch to shipped feature.",
    posted: "2026-03-13",
    source: "Company site",
  },
  {
    title: "Junior Software Developer",
    company: "Brightfield Systems",
    location: "Berlin",
    lang: "en",
    type: "Full-time",
    seniority: "Junior",
    experience: "0-2 years",
    skills: ["Go", "SQL", "Git"],
    blurb:
      "A friendly team, a clear mentoring plan and real responsibility from week one. Brightfield Systems builds logistics software used by carriers across Germany.",
    posted: "2026-03-14",
    source: "Job board",
  },
  {
    title: "Technical Writer (Developer Documentation)",
    company: "Tandem Retail",
    location: "Edinburgh, UK",
    lang: "en",
    type: "Contract",
    seniority: "Mid-level",
    experience: "2+ years",
    skills: ["Markdown", "OpenAPI", "Git"],
    blurb:
      "Tandem Retail is rebuilding its public API. We need a writer who can turn specifications into guides that developers actually enjoy reading.",
    posted: "2026-03-14",
    source: "Job board",
  },
  {
    title: "Cloud-Architekt (m/w/d)",
    company: "Rheintal Digital GmbH",
    location: "Köln",
    lang: "de",
    type: "Vollzeit",
    seniority: "Lead",
    experience: "8 Jahre",
    skills: ["AWS", "Terraform", "Architektur", "Security"],
    blurb:
      "Rheintal Digital begleitet Unternehmen in die Cloud. Als Architekt entwirfst du Zielarchitekturen, berätst Kunden und gibst dein Wissen im Team weiter.",
    posted: "2026-03-15",
    source: "Karriereseite",
  },
  {
    title: "Mobile Developer Android",
    company: "Kestrel Mobility",
    location: "Wien, Österreich",
    lang: "de",
    type: "Teilzeit",
    seniority: "Mittel",
    experience: "3 Jahre",
    skills: ["Kotlin", "Android", "Jetpack Compose"],
    blurb:
      "Kestrel Mobility entwickelt eine App für nachhaltige Mobilität. Du baust gemeinsam mit zwei weiteren Entwicklern die Android-Version weiter aus.",
    posted: "2026-03-15",
    source: "Job board",
  },
  {
    title: "Security Engineer",
    company: "Moorlight Cloud Services",
    location: "Stuttgart",
    lang: "en",
    type: "Full-time",
    seniority: "Senior",
    experience: "5+ years",
    skills: ["Threat modelling", "Python", "Linux", "Cloud security"],
    blurb:
      "Help us keep customer workloads safe. You will review designs, automate checks in the delivery pipeline and respond to the occasional incident.",
    posted: "2026-03-16",
    source: "Company site",
  },
  {
    title: "Data Analyst Reporting (m/w/d)",
    company: "Hanseatic Retail Group",
    location: "Hamburg",
    lang: "de",
    type: "Vollzeit",
    seniority: "Junior",
    experience: "1 Jahr",
    skills: ["SQL", "Power BI", "Excel"],
    blurb:
      "Die Hanseatic Retail Group sucht Unterstützung für das zentrale Reporting. Du bereitest Kennzahlen auf und erklärst sie den Fachbereichen verständlich.",
    posted: "2026-03-16",
    source: "Karriereseite",
  },
  {
    title: "Engineering Manager Platform",
    company: "Lumenwork GmbH",
    location: "Hamburg",
    lang: "en",
    type: "Full-time",
    seniority: "Lead",
    experience: "7+ years",
    skills: ["People management", "Rust", "Distributed systems"],
    blurb:
      "You will lead a team of six engineers who build the shared platform behind all Lumenwork products, balancing delivery, growth and technical direction.",
    posted: "2026-03-17",
    source: "Company site",
  },
  {
    title: "QA Engineer Mobile Testing",
    company: "Kestrel Mobility",
    location: "Wien, Österreich",
    lang: "de",
    type: "Vollzeit",
    seniority: "Mittel",
    experience: "2 Jahre",
    skills: ["Appium", "Kotlin", "Testkonzepte"],
    blurb:
      "Du verantwortest die Qualität unserer Mobile-Apps und baust die automatisierten Tests für Android und iOS aus.",
    posted: "2026-03-17",
    source: "Job board",
  },
  {
    title: "Systems Programmer C++ (Industrial Controls)",
    company: "Ironvale Automation",
    location: "Tampere, Finland",
    lang: "en",
    type: "Full-time",
    seniority: "Senior",
    experience: "6+ years",
    skills: ["C++", "Linux", "Real-time systems"],
    blurb:
      "Work on the control software that coordinates dozens of motor drives in a production line. Latency, determinism and clean interfaces matter here.",
    posted: "2026-03-18",
    source: "Company site",
  },
];

function describe(spec: Spec): string {
  const skills = spec.skills;
  if (spec.lang === "de") {
    return [
      spec.blurb,
      "",
      "Deine Aufgaben",
      `- Du entwickelst und betreibst Lösungen mit ${skills.slice(0, 2).join(" und ")}.`,
      "- Du arbeitest eng mit Fachbereichen, Produktmanagement und Kolleginnen und Kollegen zusammen.",
      "- Du sorgst für Qualität durch Code-Reviews, Tests und saubere Dokumentation.",
      "- Du bringst Ideen ein, wie wir Abläufe und Technik gemeinsam verbessern können.",
      "",
      "Dein Profil",
      `- Du bringst ${spec.experience} Berufserfahrung in einer vergleichbaren Rolle mit.`,
      `- Du kennst dich mit ${skills.join(", ")} aus.`,
      "- Du kommunizierst klar, arbeitest selbstständig und im Team.",
      "- Sehr gute Deutschkenntnisse, gute Englischkenntnisse.",
      "",
      "Wir bieten",
      "- Flexible Arbeitszeiten und die Möglichkeit zum mobilen Arbeiten.",
      "- 30 Tage Urlaub, Weiterbildungsbudget und einen Zuschuss zum Deutschlandticket.",
      "- Ein respektvolles Team und kurze Entscheidungswege.",
      "",
      "Wir freuen uns auf deine Bewerbung mit Lebenslauf und frühestem Eintrittstermin.",
    ].join("\n");
  }
  return [
    spec.blurb,
    "",
    "What you will do",
    `- Design, build and operate solutions using ${skills.slice(0, 2).join(" and ")}.`,
    "- Work closely with product managers, designers and fellow engineers.",
    "- Keep quality high through code review, automated tests and clear documentation.",
    "- Suggest ways to improve how we work and what we build.",
    "",
    "What we are looking for",
    `- ${spec.experience} of professional experience in a comparable role.`,
    `- Hands-on knowledge of ${skills.join(", ")}.`,
    "- Clear written and spoken communication; comfortable working independently and in a team.",
    "- Good English; German is a plus.",
    "",
    "What we offer",
    "- Flexible hours and hybrid or remote working.",
    "- 28 days of paid leave, a learning budget and modern equipment.",
    "- A friendly, respectful team and short decision paths.",
    "",
    "Please apply with your CV and your earliest start date.",
  ].join("\n");
}

const jobs: Map<string, JobDetailDto> = new Map();

for (const [index, spec] of seed(SPECS).entries()) {
  const id = uuid(2, index + 1);
  jobs.set(id, {
    job: {
      id,
      revision: "1",
      title: spec.title,
      company: spec.company,
      location: spec.location,
      description: describe(spec),
      sourceLabel: spec.source,
      employmentType: spec.type,
      seniority: spec.seniority,
      experienceRequirement: spec.experience,
      postedAt: spec.posted,
      sourceMethod: "text",
    },
    skills: spec.skills,
  });
}

/** Job ids in list order, for the other features (matching, generation). */
export function jobIds(): string[] {
  return [...jobs.keys()];
}

/** One stored job, or undefined. */
export function jobDetail(id: string): JobDetailDto | undefined {
  return jobs.get(id);
}

/** Saves a job built outside this file (the Arbeitnow import) and returns its detail. */
export function storeJob(
  fields: Omit<JobFieldsDto, "id" | "revision">,
  skills: string[],
): JobDetailDto {
  const id = freshId();
  const detail: JobDetailDto = { job: { ...fields, id, revision: "1" }, skills };
  jobs.set(id, detail);
  return detail;
}

/** Finds a stored job with the same description, as the Rust build dedupes captures. */
export function jobWithDescription(description: string): JobDetailDto | undefined {
  return [...jobs.values()].find((entry) => entry.job.description === description);
}

/** Removes a job; used by the deletion handlers. Returns whether a row existed. */
export function removeJob(id: string): boolean {
  return jobs.delete(id);
}

function summaryOf(detail: JobDetailDto): JobSummaryDto {
  const { id, title, company, location } = detail.job;
  return { id, title, company, location };
}

function requireJob(id: unknown): JobDetailDto {
  return jobs.get(String(id)) ?? fail("not_found", "jobs.error.notFound");
}

function capture(text: string, method: string): { status: string; job: JobDetailDto } {
  const trimmed = text.trim();
  if (trimmed === "") {
    fail("validation_error", "jobs.error.validation");
  }
  const known = jobWithDescription(text);
  if (known) {
    return { status: "reused_existing_job", job: structuredClone(known) };
  }
  const lines = trimmed.split("\n").map((line) => line.trim());
  const detail = storeJob(
    {
      title: (lines[0] ?? "").slice(0, 80) || "Untitled posting",
      company: "Not recognised",
      location: "",
      description: text,
      sourceLabel: "Pasted text",
      employmentType: "",
      seniority: "",
      experienceRequirement: "",
      postedAt: nowIso().slice(0, 10),
      sourceMethod: method,
    },
    [],
  );
  return { status: "created_job", job: structuredClone(detail) };
}

export const jobHandlers: HandlerMap = {
  list_jobs: ({ cursor }: Args) => {
    const { items, nextCursor } = pageOf([...jobs.values()].map(summaryOf), cursor, 10);
    return { jobs: items, nextCursor };
  },
  get_job: ({ jobId }: Args) => structuredClone(requireJob(jobId)),
  add_job_from_text: (args: Args) => capture(String(requestOf(args).text ?? ""), "text"),
  job_link_capabilities_get: () => ({ jobLink: true }),
  job_add_link: ({ url }: Args) => {
    if (!/^https?:\/\//.test(String(url ?? ""))) {
      fail("validation_error", "jobs.error.validation");
    }
    return capture(
      `Platform Engineer (captured from link)\nExample Cloudworks\nA synthetic posting captured from a link on ${nowIso()}.\n- Build internal tooling.\n- Support product teams.`,
      "link",
    );
  },
  search_jobs: (args: Args): JobSearchDto => {
    const query = String(requestOf(args).query ?? "");
    const tokens = tokensOf(query);
    const matches = [...jobs.values()].filter((entry) => {
      const haystack =
        `${entry.job.title} ${entry.job.company} ${entry.job.location} ${entry.job.description} ${entry.skills.join(" ")}`.toLowerCase();
      return tokens.length > 0 && tokens.every((token) => haystack.includes(token));
    });
    return {
      query,
      totalMatches: String(matches.length),
      matchedCount: String(matches.length),
      hasMore: false,
      returnedCount: matches.length,
      jobs: matches.map((entry) => ({ job: summaryOf(entry) })),
    };
  },
  update_job: (args: Args) => {
    const request = requestOf(args);
    const current = requireJob(args.jobId);
    if (current.job.revision !== String(args.expectedRevision)) {
      fail("conflict", "jobs.error.conflict");
    }
    const fields: Record<string, unknown> = { ...current.job };
    for (const [key, value] of Object.entries(request)) {
      if (key !== "skillsText" && key in current.job && key !== "id" && key !== "revision") {
        fields[key] = String(value);
      }
    }
    const skills =
      typeof request.skillsText === "string"
        ? request.skillsText
            .split("\n")
            .map((line) => line.trim())
            .filter((line) => line !== "")
        : current.skills;
    const next: JobDetailDto = {
      job: { ...(fields as JobFieldsDto), revision: nextRevision(current.job.revision) },
      skills,
    };
    jobs.set(current.job.id, next);
    return structuredClone(next);
  },
};
