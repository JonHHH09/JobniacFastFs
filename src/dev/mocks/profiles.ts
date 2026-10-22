import type {
  ContactDto,
  EducationDto,
  ExperienceDto,
  LanguageDto,
  LinkDto,
  PersonalDetailsEnvelopeDto,
  ProfileGraphDto,
  ProfileIdentityDto,
  ProfileSearchMatchDto,
  ProjectDto,
  SkillDto,
} from "../../ipc/bindings";
import {
  type Args,
  FIXTURE_TIME,
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

// Synthetic people only: fictional names, example.com addresses, example.org links.

type Spec = {
  name: string;
  summary: string;
  skills: [string, string][];
  languages: [string, string][];
  education: [string, string, string, string, string, string][]; // institution, degree, field, location, start, end
  experiences: [string, string, string, string, string | null, string][]; // company, title, location, start, end, description
  projects: [string, string, string, string[]][]; // name, url, description, technologies
};

const SPECS: Spec[] = [
  {
    name: "Mara Teller",
    summary:
      "Backend engineer with eight years of experience building data-heavy services in Rust and Go. Enjoys small teams, clear interfaces and boring, reliable infrastructure.",
    skills: [
      ["Rust", "Languages"],
      ["Go", "Languages"],
      ["PostgreSQL", "Data"],
      ["Kubernetes", "Infrastructure"],
      ["gRPC", "Protocols"],
    ],
    languages: [
      ["German", "Native"],
      ["English", "C1"],
    ],
    education: [
      [
        "Northgate University of Applied Sciences",
        "M.Sc.",
        "Computer Science",
        "Hamburg",
        "2012-10",
        "2015-03",
      ],
    ],
    experiences: [
      [
        "Lumenwork GmbH",
        "Senior Backend Engineer",
        "Hamburg",
        "2020-04",
        null,
        "Lead developer of the ingestion pipeline (2 TB/day). Introduced Rust for the hot path and cut p99 latency from 480 ms to 70 ms.",
      ],
      [
        "Brightfield Systems",
        "Software Engineer",
        "Berlin",
        "2016-02",
        "2020-03",
        "Built REST and gRPC services for a logistics platform; owned the PostgreSQL schema and its migrations.",
      ],
    ],
    projects: [
      [
        "pagecache",
        "https://example.org/projects/pagecache",
        "An embeddable read-through cache with deterministic eviction.",
        ["Rust", "Tokio"],
      ],
    ],
  },
  {
    name: "Jonas Kellner",
    summary:
      "Frontend developer focused on accessible, fast interfaces. Seven years with React and TypeScript, a taste for design systems and a habit of measuring before optimising.",
    skills: [
      ["TypeScript", "Languages"],
      ["React", "Frameworks"],
      ["CSS", "Languages"],
      ["Accessibility", "Practices"],
      ["Vite", "Tooling"],
    ],
    languages: [
      ["German", "Native"],
      ["English", "C2"],
      ["Spanish", "A2"],
    ],
    education: [
      ["Eastbridge University", "B.Sc.", "Media Informatics", "Leipzig", "2013-10", "2017-02"],
    ],
    experiences: [
      [
        "Pixelharbor AG",
        "Frontend Lead",
        "Remote",
        "2021-01",
        null,
        "Leads four developers on a customer portal used by 120k people. Built the in-house design system and its accessibility test suite.",
      ],
      [
        "Cobalt Studio",
        "Web Developer",
        "Leipzig",
        "2017-04",
        "2020-12",
        "Delivered marketing sites and shop front-ends for agency clients.",
      ],
    ],
    projects: [
      [
        "tokenbook",
        "https://example.org/projects/tokenbook",
        "A living style guide that renders design tokens as documentation.",
        ["TypeScript", "React", "Storybook"],
      ],
      [
        "focus-trap-lite",
        "https://example.org/projects/focus-trap-lite",
        "A 2 kB focus management helper for modal dialogs.",
        ["TypeScript"],
      ],
    ],
  },
  {
    name: "Priya Raman",
    summary:
      "Data engineer and analytics lead. Designs batch and streaming pipelines, mentors analysts and cares about data quality contracts.",
    skills: [
      ["Python", "Languages"],
      ["SQL", "Data"],
      ["Apache Spark", "Data"],
      ["Airflow", "Tooling"],
      ["dbt", "Tooling"],
    ],
    languages: [
      ["English", "Native"],
      ["Hindi", "Native"],
      ["German", "B1"],
    ],
    education: [
      [
        "Harlow Institute of Technology",
        "B.Tech.",
        "Information Technology",
        "Pune",
        "2010-08",
        "2014-05",
      ],
      ["Westmere University", "M.Sc.", "Data Science", "Edinburgh", "2015-09", "2016-09"],
    ],
    experiences: [
      [
        "Quillstone Analytics",
        "Lead Data Engineer",
        "Munich",
        "2019-06",
        null,
        "Owns the warehouse and 300 dbt models. Introduced data contracts and freshness alerts; reduced incident count by 60 percent.",
      ],
      [
        "Tandem Retail",
        "Data Engineer",
        "Edinburgh",
        "2016-11",
        "2019-05",
        "Migrated nightly ETL jobs to Spark on a managed cluster.",
      ],
    ],
    projects: [
      [
        "contractcheck",
        "https://example.org/projects/contractcheck",
        "A command line tool that validates tables against YAML data contracts.",
        ["Python", "SQL"],
      ],
    ],
  },
  {
    name: "Tomas Brandt",
    summary:
      "DevOps engineer who treats infrastructure as software. Experienced with Terraform, GitHub Actions and on-call rotations that let people sleep.",
    skills: [
      ["Terraform", "Infrastructure"],
      ["Linux", "Systems"],
      ["GitHub Actions", "Tooling"],
      ["Prometheus", "Observability"],
      ["Bash", "Languages"],
    ],
    languages: [
      ["German", "Native"],
      ["English", "B2"],
    ],
    education: [
      [
        "Fernwald Technical College",
        "Diploma",
        "Systems Administration",
        "Stuttgart",
        "2008-09",
        "2011-07",
      ],
    ],
    experiences: [
      [
        "Moorlight Cloud Services",
        "Site Reliability Engineer",
        "Stuttgart",
        "2018-03",
        null,
        "Runs 40 production clusters. Wrote the golden-path Terraform modules adopted by 12 teams.",
      ],
      [
        "Kestrel Hosting",
        "Systems Administrator",
        "Stuttgart",
        "2011-09",
        "2018-02",
        "Operated a fleet of 600 virtual machines and automated patching.",
      ],
    ],
    projects: [],
  },
  {
    name: "Lena Okoye",
    summary:
      "Product-minded full-stack developer, comfortable from SQL schema to pixel. Has shipped two products from zero to first paying customers.",
    skills: [
      ["Python", "Languages"],
      ["Django", "Frameworks"],
      ["React", "Frameworks"],
      ["PostgreSQL", "Data"],
      ["Docker", "Infrastructure"],
    ],
    languages: [
      ["English", "Native"],
      ["French", "B2"],
    ],
    education: [
      ["Ashford College", "B.A.", "Economics", "Bristol", "2011-09", "2014-06"],
      [
        "Open Learning Collective",
        "Certificate",
        "Web Development",
        "Online",
        "2015-01",
        "2015-08",
      ],
    ],
    experiences: [
      [
        "Fieldnote Ltd",
        "Co-founder and CTO",
        "Bristol",
        "2019-02",
        "2024-06",
        "Built a note-taking app for field researchers from prototype to 3k paying users. Hired and managed a team of five.",
      ],
      [
        "Greenlane Digital",
        "Full-stack Developer",
        "London",
        "2015-10",
        "2019-01",
        "Delivered client web applications in Django and React.",
      ],
    ],
    projects: [
      [
        "fieldnote-sync",
        "https://example.org/projects/fieldnote-sync",
        "Offline-first synchronisation library with conflict-free merging.",
        ["Python", "SQLite"],
      ],
    ],
  },
  {
    name: "Sven Aalto",
    summary:
      "Embedded software engineer for industrial controllers. Fluent in C and C++, fond of hardware-in-the-loop testing and well-written datasheets.",
    skills: [
      ["C", "Languages"],
      ["C++", "Languages"],
      ["RTOS", "Systems"],
      ["CAN bus", "Protocols"],
      ["Python", "Languages"],
    ],
    languages: [
      ["Finnish", "Native"],
      ["English", "C1"],
      ["German", "B1"],
    ],
    education: [
      ["Lakeside University", "M.Sc.", "Electrical Engineering", "Tampere", "2009-09", "2014-06"],
    ],
    experiences: [
      [
        "Ironvale Automation",
        "Senior Embedded Engineer",
        "Tampere",
        "2017-08",
        null,
        "Firmware for motor controllers on Cortex-M. Built the hardware-in-the-loop rig that runs 4,000 regression cases nightly.",
      ],
    ],
    projects: [
      [
        "canscope",
        "https://example.org/projects/canscope",
        "A CAN bus trace viewer with signal decoding from DBC files.",
        ["C++", "Qt"],
      ],
    ],
  },
  {
    name: "Chloe Marchetti",
    summary:
      "Machine learning engineer working on ranking and recommendations. Prefers simple baselines, solid evaluation and reproducible experiments.",
    skills: [
      ["Python", "Languages"],
      ["PyTorch", "ML"],
      ["scikit-learn", "ML"],
      ["MLflow", "Tooling"],
      ["SQL", "Data"],
    ],
    languages: [
      ["Italian", "Native"],
      ["English", "C2"],
    ],
    education: [["Riverside Polytechnic", "M.Sc.", "Statistics", "Milan", "2013-09", "2016-03"]],
    experiences: [
      [
        "Stagecoach Media",
        "ML Engineer",
        "Amsterdam",
        "2019-01",
        null,
        "Ranking models for a video catalogue; improved watch-through by 4.1 percent in a controlled experiment.",
      ],
      [
        "Piazza Labs",
        "Data Scientist",
        "Milan",
        "2016-05",
        "2018-12",
        "Demand forecasting for a grocery chain.",
      ],
    ],
    projects: [],
  },
  {
    name: "Daniel Hofer",
    summary:
      "Quality engineer and test automation specialist. Builds pragmatic test pyramids and keeps flaky tests out of the main branch.",
    skills: [
      ["Java", "Languages"],
      ["Selenium", "Testing"],
      ["Playwright", "Testing"],
      ["Jenkins", "Tooling"],
    ],
    languages: [
      ["German", "Native"],
      ["English", "B2"],
    ],
    education: [
      [
        "Alpenland University of Applied Sciences",
        "B.Sc.",
        "Software Engineering",
        "Graz",
        "2010-10",
        "2013-09",
      ],
    ],
    experiences: [
      [
        "Edelweiss Software",
        "Test Automation Lead",
        "Graz",
        "2016-03",
        null,
        "Owns the end-to-end suite (900 scenarios, 12 minutes). Introduced test impact analysis in the pipeline.",
      ],
    ],
    projects: [],
  },
];

const profileGraphs: Map<string, ProfileGraphDto> = new Map();
let counter = 100;

/** The fixtures list months (`2020-04`); the editor and Rust only accept a full `YYYY-MM-DD`. */
function isoDate(value: string | null): string | null {
  return value !== null && /^\d{4}-\d{2}$/.test(value) ? `${value}-01` : value;
}

function nextChildId(): string {
  counter += 1;
  return uuid(0x20, counter);
}

function expand(id: string, spec: Spec, index: number): ProfileGraphDto {
  const slug = spec.name.toLowerCase().replace(/[^a-z]+/g, ".");
  const email = `${slug}@example.com`;
  const contacts: ContactDto[] = [
    { id: nextChildId(), contactType: "email", contactValue: email, label: "Work" },
    {
      id: nextChildId(),
      contactType: "phone",
      // Ofcom's range reserved for drama and fiction, so no real subscriber can be hit.
      contactValue: `+44 20 7946 0${100 + index}`,
      label: "Mobile",
    },
  ];
  const links: LinkDto[] = [
    {
      id: nextChildId(),
      linkType: "github",
      url: `https://example.org/${slug}`,
      label: "Code",
    },
  ];
  const skills: SkillDto[] = spec.skills.map(([skill, category], i) => ({
    id: nextChildId(),
    skill,
    normalizedSkill: skill.toLowerCase(),
    category,
    displayOrder: i,
  }));
  const languages: LanguageDto[] = spec.languages.map(([language, proficiency], i) => ({
    id: nextChildId(),
    language,
    normalizedLanguage: language.toLowerCase(),
    proficiency,
    displayOrder: i,
  }));
  const education: EducationDto[] = spec.education.map(
    ([institution, degree, field, location, startDate, endDate]) => ({
      id: nextChildId(),
      institution,
      degree,
      field,
      location,
      startDate: isoDate(startDate),
      endDate: isoDate(endDate),
      relevantFocus: null,
    }),
  );
  const experiences: ExperienceDto[] = spec.experiences.map(
    ([company, title, location, startDate, endDate, description], i) => ({
      id: nextChildId(),
      company,
      title,
      location,
      startDate: isoDate(startDate),
      endDate: isoDate(endDate),
      description,
      displayOrder: i,
    }),
  );
  const projects: ProjectDto[] = spec.projects.map(([name, url, description, technologies], i) => ({
    id: nextChildId(),
    name,
    url,
    description,
    displayOrder: i,
    technologies: technologies.map((technology, t) => ({
      id: nextChildId(),
      technology,
      normalizedTechnology: technology.toLowerCase(),
      displayOrder: t,
    })),
  }));
  return {
    profile: {
      id,
      fullName: spec.name,
      email,
      summary: spec.summary,
      revision: "1",
      createdAt: FIXTURE_TIME,
      updatedAt: FIXTURE_TIME,
    },
    contacts,
    links,
    skills,
    languages,
    education,
    experiences,
    projects,
  };
}

for (const [index, spec] of seed(SPECS).entries()) {
  const id = uuid(1, index + 1);
  profileGraphs.set(id, expand(id, spec, index));
}

const personalDetails = new Map<string, PersonalDetailsEnvelopeDto["personalDetails"]>();

/** Profile ids in list order, for the other features (matching, generation). */
export function profileIds(): string[] {
  return [...profileGraphs.keys()];
}

/** One stored profile graph, or undefined. */
export function profileGraph(id: string): ProfileGraphDto | undefined {
  return profileGraphs.get(id);
}

function identityOf(graph: ProfileGraphDto): ProfileIdentityDto {
  return { id: graph.profile.id, fullName: graph.profile.fullName, email: graph.profile.email };
}

function requireProfile(id: unknown): ProfileGraphDto {
  const graph = profileGraphs.get(String(id));
  return graph ?? fail("not_found", "profile.error.notFound");
}

function checkRevision(graph: ProfileGraphDto, expected: unknown): void {
  if (graph.profile.revision !== String(expected)) {
    fail("conflict", "profile.error.conflict");
  }
}

type Row = Record<string, unknown>;

function rowsOf(value: unknown): Row[] {
  return Array.isArray(value) ? (value as Row[]) : [];
}

function text(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/** Rebuilds a graph from a write request, keeping the ids the request names and minting the rest. */
function applyWrite(graph: ProfileGraphDto, request: Row): ProfileGraphDto {
  const id = (row: Row): string => text(row.id) ?? freshId();
  const order = (row: Row, i: number): number =>
    typeof row.displayOrder === "number" ? row.displayOrder : i;
  const next: ProfileGraphDto = { ...graph, profile: { ...graph.profile } };
  if (typeof request.fullName === "string") next.profile.fullName = request.fullName;
  if (typeof request.email === "string") next.profile.email = request.email;
  if ("summary" in request) next.profile.summary = text(request.summary);
  if (request.contacts !== undefined) {
    next.contacts = rowsOf(request.contacts).map((r) => ({
      id: id(r),
      contactType: text(r.contactType) ?? "",
      contactValue: text(r.contactValue) ?? "",
      label: text(r.label),
    }));
  }
  if (request.links !== undefined) {
    next.links = rowsOf(request.links).map((r) => ({
      id: id(r),
      linkType: text(r.linkType) ?? "",
      url: text(r.url) ?? "",
      label: text(r.label),
    }));
  }
  if (request.skills !== undefined) {
    next.skills = rowsOf(request.skills).map((r, i) => ({
      id: id(r),
      skill: text(r.skill) ?? "",
      normalizedSkill: (text(r.normalizedSkill) ?? text(r.skill) ?? "").toLowerCase(),
      category: text(r.category),
      displayOrder: order(r, i),
    }));
  }
  if (request.languages !== undefined) {
    next.languages = rowsOf(request.languages).map((r, i) => ({
      id: id(r),
      language: text(r.language) ?? "",
      normalizedLanguage: (text(r.normalizedLanguage) ?? text(r.language) ?? "").toLowerCase(),
      proficiency: text(r.proficiency),
      displayOrder: order(r, i),
    }));
  }
  if (request.education !== undefined) {
    next.education = rowsOf(request.education).map((r) => ({
      id: id(r),
      institution: text(r.institution),
      degree: text(r.degree),
      field: text(r.field),
      location: text(r.location),
      startDate: text(r.startDate),
      endDate: text(r.endDate),
      relevantFocus: text(r.relevantFocus),
    }));
  }
  if (request.experiences !== undefined) {
    next.experiences = rowsOf(request.experiences).map((r, i) => ({
      id: id(r),
      company: text(r.company),
      title: text(r.title),
      location: text(r.location),
      startDate: text(r.startDate),
      endDate: text(r.endDate),
      description: text(r.description),
      displayOrder: order(r, i),
    }));
  }
  if (request.projects !== undefined) {
    next.projects = rowsOf(request.projects).map((r, i) => ({
      id: id(r),
      name: text(r.name),
      url: text(r.url),
      description: text(r.description),
      displayOrder: order(r, i),
      technologies: rowsOf(r.technologies).map((t, k) => ({
        id: id(t),
        technology: text(t.technology) ?? "",
        normalizedTechnology: (
          text(t.normalizedTechnology) ??
          text(t.technology) ??
          ""
        ).toLowerCase(),
        displayOrder: order(t, k),
      })),
    }));
  }
  return next;
}

function validateWrite(request: Row): void {
  if (typeof request.fullName !== "string" || request.fullName.trim() === "") {
    fail("validation_error", "profile.error.validation");
  }
  if (typeof request.email !== "string" || !request.email.includes("@")) {
    fail("validation_error", "profile.error.validation");
  }
}

function searchText(graph: ProfileGraphDto): Record<string, string> {
  return {
    name: graph.profile.fullName,
    email: graph.profile.email,
    summary: graph.profile.summary ?? "",
    skills: graph.skills.map((s) => s.skill).join(" "),
    experience: graph.experiences.map((e) => `${e.title} ${e.company}`).join(" "),
  };
}

export const profileHandlers: HandlerMap = {
  list_profiles: ({ cursor }: Args) => {
    const identities = [...profileGraphs.values()].map(identityOf);
    const { items, nextCursor } = pageOf(identities, cursor, 5);
    return { profiles: items, nextCursor };
  },
  get_profile: ({ profileId }: Args) => structuredClone(requireProfile(profileId)),
  create_profile: (args: Args) => {
    const request = requestOf(args);
    validateWrite(request);
    const id = freshId();
    const now = nowIso();
    const base: ProfileGraphDto = {
      profile: {
        id,
        fullName: "",
        email: "",
        summary: null,
        revision: "1",
        createdAt: now,
        updatedAt: now,
      },
      contacts: [],
      links: [],
      skills: [],
      languages: [],
      education: [],
      experiences: [],
      projects: [],
    };
    const graph = applyWrite(base, request);
    profileGraphs.set(id, graph);
    return structuredClone(graph);
  },
  update_profile: (args: Args) => {
    const request = requestOf(args);
    const graph = requireProfile(args.profileId);
    checkRevision(graph, args.expectedRevision);
    validateWrite(request);
    const next = applyWrite(graph, request);
    next.profile.revision = nextRevision(graph.profile.revision);
    next.profile.updatedAt = nowIso();
    profileGraphs.set(graph.profile.id, next);
    return structuredClone(next);
  },
  update_project: (args: Args) => {
    const request = requestOf(args);
    const graph = requireProfile(request.profileId);
    checkRevision(graph, request.expectedRevision);
    const index = graph.projects.findIndex((p) => p.id === request.projectId);
    if (index < 0) {
      fail("not_found", "profile.error.notFound");
    }
    const project = { ...graph.projects[index] };
    if ("name" in request) project.name = text(request.name);
    if ("url" in request) project.url = text(request.url);
    if ("description" in request) project.description = text(request.description);
    const projects = [...graph.projects];
    projects[index] = project;
    const revision = nextRevision(graph.profile.revision);
    profileGraphs.set(graph.profile.id, {
      ...graph,
      projects,
      profile: { ...graph.profile, revision, updatedAt: nowIso() },
    });
    return { project: structuredClone(project), profileRevision: revision };
  },
  search_profiles: ({ query }: Args) => {
    const tokens = tokensOf(String(query ?? ""));
    const matches: ProfileSearchMatchDto[] = [];
    for (const graph of profileGraphs.values()) {
      const fields = searchText(graph);
      const matched = new Set<string>();
      let score = 0;
      for (const token of tokens) {
        for (const [field, value] of Object.entries(fields)) {
          if (value.toLowerCase().includes(token)) {
            matched.add(field);
            score += 1;
          }
        }
      }
      if (tokens.length > 0 && matched.size > 0) {
        matches.push({
          profile: identityOf(graph),
          matchedFields: [...matched],
          score: score.toFixed(1),
        });
      }
    }
    matches.sort((a, b) => Number(b.score) - Number(a.score));
    return {
      query: String(query ?? ""),
      queryTokens: tokens,
      totalMatches: String(matches.length),
      matchedCount: String(matches.length),
      hasMore: false,
      returnedCount: matches.length,
      profiles: matches,
    };
  },
  get_personal_details: ({ profileId }: Args) => {
    const graph = requireProfile(profileId);
    return {
      profileId: graph.profile.id,
      profileRevision: graph.profile.revision,
      profileUpdatedAt: graph.profile.updatedAt,
      personalDetails: personalDetails.get(graph.profile.id) ?? null,
    } satisfies PersonalDetailsEnvelopeDto;
  },
  update_personal_details: (args: Args) => {
    const request = requestOf(args);
    const graph = requireProfile(args.profileId);
    checkRevision(graph, args.expectedRevision);
    const now = nowIso();
    const previous = personalDetails.get(graph.profile.id);
    personalDetails.set(graph.profile.id, {
      profileId: graph.profile.id,
      dateOfBirth: text(request.dateOfBirth),
      nationality: text(request.nationality),
      photoDocumentId: text(request.photoDocumentId),
      createdAt: previous?.createdAt ?? now,
      updatedAt: now,
    });
    const revision = nextRevision(graph.profile.revision);
    profileGraphs.set(graph.profile.id, {
      ...graph,
      profile: { ...graph.profile, revision, updatedAt: now },
    });
    return {
      profileId: graph.profile.id,
      profileRevision: revision,
      profileUpdatedAt: now,
      personalDetails: personalDetails.get(graph.profile.id) ?? null,
    } satisfies PersonalDetailsEnvelopeDto;
  },
};

/** Removes a profile; used by the deletion handlers. Returns whether a row existed. */
export function removeProfile(id: string): boolean {
  personalDetails.delete(id);
  return profileGraphs.delete(id);
}
