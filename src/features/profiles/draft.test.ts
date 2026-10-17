import { describe, expect, it } from "vitest";
import type { PersonalDetailsEnvelopeDto, ProfileGraphDto } from "../../ipc/bindings";
import {
  addRow,
  addTechnology,
  blankDraft,
  canSaveProject,
  draftFromGraph,
  isProfileDirty,
  nextDisplayOrder,
  personalWriteRequest,
  projectPatchRequest,
  removeRow,
  setRowField,
  setTechnology,
  toWriteRequest,
} from "./draft";
import { editorReducer, hasUnsavedChanges, initialEditorState } from "./editorState";
import { collectionSections } from "./schema";

// Synthetic identifiers and values only.
const PROFILE_ID = "00000000-0000-4000-8000-000000000001";
const PROJECT_ID = "00000000-0000-4000-8000-000000000002";
const TECHNOLOGY_ID = "00000000-0000-4000-8000-000000000003";
const EXPERIENCE_ID = "00000000-0000-4000-8000-000000000004";

function fixtureGraph(): ProfileGraphDto {
  return {
    profile: {
      id: PROFILE_ID,
      fullName: "Ada Example",
      email: "ada@example.test",
      summary: "Analyst",
      revision: "7",
      createdAt: "2026-01-02T03:04:05Z",
      updatedAt: "2026-01-02T03:04:05Z",
    },
    contacts: [],
    links: [],
    skills: [],
    languages: [],
    education: [],
    experiences: [
      {
        id: EXPERIENCE_ID,
        company: "Example Labs",
        title: "Engineer",
        location: null,
        startDate: "2020-01-01",
        endDate: null,
        description: null,
        displayOrder: 0,
      },
    ],
    projects: [
      {
        id: PROJECT_ID,
        name: "Synthetic project",
        url: null,
        description: null,
        displayOrder: 0,
        technologies: [
          {
            id: TECHNOLOGY_ID,
            technology: "Rust",
            normalizedTechnology: "rust",
            displayOrder: 0,
          },
        ],
      },
    ],
  };
}

describe("draft shape", () => {
  it("keeps every collection provided on a read and uses only the write keys", () => {
    const draft = draftFromGraph(fixtureGraph());
    const request = toWriteRequest(draft);

    expect(Object.keys(request)).toEqual([
      "fullName",
      "email",
      "summary",
      "contacts",
      "links",
      "skills",
      "languages",
      "education",
      "experiences",
      "projects",
    ]);
    expect(request.experiences).toEqual([
      {
        id: EXPERIENCE_ID,
        company: "Example Labs",
        title: "Engineer",
        location: null,
        startDate: "2020-01-01",
        endDate: null,
        description: null,
        displayOrder: 0,
      },
    ]);
    expect(request.projects).toEqual([
      {
        id: PROJECT_ID,
        name: "Synthetic project",
        url: null,
        description: null,
        displayOrder: 0,
        technologies: [
          { id: TECHNOLOGY_ID, technology: "Rust", normalizedTechnology: "rust", displayOrder: 0 },
        ],
      },
    ]);
  });

  it("omits a collection that the draft does not provide", () => {
    const request = toWriteRequest({
      fullName: "",
      email: "",
      summary: "",
      collections: {},
    });
    expect(request).toEqual({ fullName: "", email: "", summary: "" });
  });

  it("follows the schema order of the write keys for every collection", () => {
    expect(collectionSections.map((section) => section.key)).toEqual([
      "contacts",
      "links",
      "skills",
      "languages",
      "education",
      "experiences",
      "projects",
    ]);
  });
});

describe("dirty state", () => {
  it("is clean for a freshly loaded draft and dirty after one field changes", () => {
    const baseline = draftFromGraph(fixtureGraph());
    expect(isProfileDirty(baseline, baseline)).toBe(false);
    const edited = { ...baseline, summary: "Updated summary" };
    expect(isProfileDirty(edited, baseline)).toBe(true);
  });
});

describe("row editing", () => {
  it("clears a date when the text is emptied and keeps other text as typed", () => {
    const draft = draftFromGraph(fixtureGraph());
    const cleared = setRowField(draft, "experiences", 0, "endDate", "");
    expect(cleared.collections.experiences?.[0]?.endDate).toBeNull();
    const typed = setRowField(draft, "experiences", 0, "company", " Spaced ");
    expect(typed.collections.experiences?.[0]?.company).toBe(" Spaced ");
  });

  it("ignores a field that the form does not edit", () => {
    const draft = draftFromGraph(fixtureGraph());
    expect(setRowField(draft, "experiences", 0, "id", "other")).toBe(draft);
    expect(setRowField(draft, "experiences", 0, "displayOrder", "9")).toBe(draft);
  });

  it("creates the collection on the first append and gives the new row the next display order", () => {
    const blank = { fullName: "", email: "", summary: "", collections: {} };
    const first = addRow(blank, "skills");
    expect(first.ok).toBe(true);
    if (!first.ok) {
      return;
    }
    expect(first.draft.collections.skills).toEqual([
      { normalizedSkill: null, category: null, displayOrder: 0 },
    ]);

    const second = addRow(first.draft, "skills");
    expect(second.ok && second.draft.collections.skills).toEqual([
      { normalizedSkill: null, category: null, displayOrder: 0 },
      { normalizedSkill: null, category: null, displayOrder: 1 },
    ]);
  });

  it("refuses an append past the display-order limit with the limit copy", () => {
    expect(nextDisplayOrder([{ displayOrder: 2147483647 }])).toBeNull();
    const draft = {
      fullName: "",
      email: "",
      summary: "",
      collections: { experiences: [{ displayOrder: 2147483647 }] },
    };
    expect(addRow(draft, "experiences")).toEqual({
      ok: false,
      messageId: "profile.displayOrder.limit",
    });
  });

  it("removes one row and leaves the others in place", () => {
    const draft = draftFromGraph(fixtureGraph());
    const withTwo = addRow(draft, "experiences");
    expect(withTwo.ok).toBe(true);
    if (!withTwo.ok) {
      return;
    }
    const removed = removeRow(withTwo.draft, "experiences", 0);
    expect(removed.collections.experiences).toHaveLength(1);
    expect(removed.collections.experiences?.[0]?.id).toBeUndefined();
  });

  it("appends a technology under a project with its own display order", () => {
    const draft = draftFromGraph(fixtureGraph());
    const result = addTechnology(draft, 0);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.draft.collections.projects?.[0]?.technologies).toEqual([
        expect.objectContaining({ id: TECHNOLOGY_ID }),
        { normalizedTechnology: null, displayOrder: 1 },
      ]);
    }
  });
});

describe("project save rules", () => {
  it("allows a save when the first changed field is a non-blank string", () => {
    const baseline = draftFromGraph(fixtureGraph());
    const draft = setRowField(baseline, "projects", 0, "name", "Renamed project");
    expect(canSaveProject(draft, baseline, 0)).toBe(true);
  });

  it("refuses a blank name and a cleared url", () => {
    const baseline = draftFromGraph(fixtureGraph());
    expect(canSaveProject(setRowField(baseline, "projects", 0, "name", "  "), baseline, 0)).toBe(
      false,
    );
    const withUrl = { ...baseline, collections: { ...baseline.collections } };
    const urlRow = { ...withUrl.collections.projects?.[0], url: "https://example.test/a" };
    const savedWithUrl = {
      ...baseline,
      collections: { ...baseline.collections, projects: [urlRow] },
    };
    expect(
      canSaveProject(setRowField(savedWithUrl, "projects", 0, "url", ""), savedWithUrl, 0),
    ).toBe(false);
  });

  it("refuses a project whose technologies changed, since they save with the profile", () => {
    const baseline = draftFromGraph(fixtureGraph());
    const draft = setRowField(baseline, "projects", 0, "name", "Renamed project");
    const changedTech = {
      ...draft,
      collections: {
        ...draft.collections,
        projects: [{ ...draft.collections.projects?.[0], technologies: [] }],
      },
    };
    expect(canSaveProject(changedTech, baseline, 0)).toBe(false);
  });

  it("builds the project patch from the changed fields and the expected revision", () => {
    const baseline = draftFromGraph(fixtureGraph());
    const draft = setRowField(baseline, "projects", 0, "name", "Renamed project");
    expect(projectPatchRequest(PROFILE_ID, draft, baseline, 0, "7")).toEqual({
      profileId: PROFILE_ID,
      projectId: PROJECT_ID,
      expectedRevision: "7",
      name: "Renamed project",
    });
  });
});

describe("personal details", () => {
  it("sends empty text as null", () => {
    expect(personalWriteRequest({ dateOfBirth: "", nationality: "" })).toEqual({
      dateOfBirth: null,
      nationality: null,
    });
    expect(personalWriteRequest({ dateOfBirth: "1990-02-03", nationality: "Example" })).toEqual({
      dateOfBirth: "1990-02-03",
      nationality: "Example",
    });
  });

  it("reads the envelope fields the form shows", () => {
    const envelope: PersonalDetailsEnvelopeDto = {
      profileId: PROFILE_ID,
      profileRevision: "7",
      profileUpdatedAt: "2026-01-02T03:04:05Z",
      personalDetails: null,
    };
    expect(personalWriteRequest({ dateOfBirth: "", nationality: "" }).dateOfBirth).toBeNull();
    expect(envelope.personalDetails).toBeNull();
  });
});

describe("normalized fields after an edit (UI-PRF-006)", () => {
  it("clears the stored normalized skill (sent as null) when the skill name is edited", () => {
    const base = draftFromGraph(fixtureGraph());
    const draft = {
      ...base,
      collections: {
        ...base.collections,
        skills: [{ id: "s1", skill: "Python", normalizedSkill: "python", displayOrder: 0 }],
      },
    };
    const edited = setRowField(draft, "skills", 0, "skill", "Rust");
    const request = toWriteRequest(edited) as { skills: Array<Record<string, unknown>> };
    expect(request.skills[0]).toEqual({
      id: "s1",
      skill: "Rust",
      normalizedSkill: null,
      displayOrder: 0,
    });
  });

  it("clears the stored normalized language (sent as null) when the language name is edited", () => {
    const base = draftFromGraph(fixtureGraph());
    const draft = {
      ...base,
      collections: {
        ...base.collections,
        languages: [
          { id: "l1", language: "German", normalizedLanguage: "german", displayOrder: 0 },
        ],
      },
    };
    const edited = setRowField(draft, "languages", 0, "language", "French");
    const request = toWriteRequest(edited) as { languages: Array<Record<string, unknown>> };
    expect(request.languages[0]).toHaveProperty("normalizedLanguage", null);
  });

  it("clears the stored normalized technology (sent as null) when the technology name is edited", () => {
    const edited = setTechnology(draftFromGraph(fixtureGraph()), 0, 0, "Go");
    const request = toWriteRequest(edited) as {
      projects: Array<{ technologies: Array<Record<string, unknown>> }>;
    };
    expect(request.projects[0]?.technologies[0]).toEqual({
      id: TECHNOLOGY_ID,
      technology: "Go",
      normalizedTechnology: null,
      displayOrder: 0,
    });
  });
});

describe("blank new profile", () => {
  it("provides every collection as an empty list", () => {
    const draft = blankDraft();
    for (const spec of collectionSections) {
      expect(draft.collections[spec.key]).toEqual([]);
    }
  });

  it("is not dirty until something is typed or added", () => {
    const started = editorReducer(initialEditorState, { type: "newProfile" });
    expect(hasUnsavedChanges(started)).toBe(false);
    const typed = editorReducer(started, { type: "setField", field: "fullName", value: "A" });
    expect(hasUnsavedChanges(typed)).toBe(true);
    const withRow = editorReducer(started, { type: "addRow", collection: "skills" });
    expect(hasUnsavedChanges(withRow)).toBe(true);
  });
});
