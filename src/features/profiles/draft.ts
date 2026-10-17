import type { MessageId } from "../../i18n/messages";
import type { PersonalDetailsEnvelopeDto, ProfileGraphDto } from "../../ipc/bindings";
import {
  type CollectionKey,
  type CollectionSpec,
  collectionSections,
  MAX_DISPLAY_ORDER,
  projectFields,
  technologyWriteKeys,
} from "./schema";

/**
 * The editable profile draft (UI state only). Rust validates and normalizes every value on save;
 * this module only shapes the data the form edits and mirrors the few checks the form shows early.
 */

/** One collection row. A row without `id` is new; `technologies` exists only on project rows. */
export type Row = { id?: string } & Record<string, unknown>;

/** The profile draft. A collection key that is absent means "not provided". */
export type ProfileDraft = {
  fullName: string;
  email: string;
  summary: string;
  collections: Partial<Record<CollectionKey, Row[]>>;
};

/** The personal-details draft: the two editable fields, as text. */
export type PersonalDraft = {
  dateOfBirth: string;
  nationality: string;
};

/** A result of an append: the new draft, or the message id that refuses it. */
export type EditResult = { ok: true; draft: ProfileDraft } | { ok: false; messageId: MessageId };

/** Picks the named keys of a generated record (keys the record lacks stay absent). */
function pick(record: object, keys: readonly string[]): Row {
  const source = record as Record<string, unknown>;
  const row: Row = {};
  for (const key of keys) {
    if (key in source) {
      row[key] = source[key];
    }
  }
  return row;
}

/** Builds the draft of a loaded or saved graph. Every collection is provided on a read. */
export function draftFromGraph(graph: ProfileGraphDto): ProfileDraft {
  const projects = graph.projects.map((project) => {
    const row = pick(project, collectionKeysOf("projects"));
    row.technologies = project.technologies.map((technology) =>
      pick(technology, technologyWriteKeys),
    );
    return row;
  });
  return {
    fullName: graph.profile.fullName,
    email: graph.profile.email,
    summary: graph.profile.summary ?? "",
    collections: {
      contacts: graph.contacts.map((row) => pick(row, collectionKeysOf("contacts"))),
      links: graph.links.map((row) => pick(row, collectionKeysOf("links"))),
      skills: graph.skills.map((row) => pick(row, collectionKeysOf("skills"))),
      languages: graph.languages.map((row) => pick(row, collectionKeysOf("languages"))),
      education: graph.education.map((row) => pick(row, collectionKeysOf("education"))),
      experiences: graph.experiences.map((row) => pick(row, collectionKeysOf("experiences"))),
      projects,
    },
  };
}

/** The write keys of one collection, from the schema. */
function collectionKeysOf(key: CollectionKey): readonly string[] {
  const spec = collectionSections.find((section) => section.key === key);
  return spec ? spec.writeKeys.filter((writeKey) => writeKey !== "technologies") : [];
}

/** A blank draft for a new profile: every collection is provided and empty . */
export function blankDraft(): ProfileDraft {
  return {
    fullName: "",
    email: "",
    summary: "",
    collections: {
      contacts: [],
      links: [],
      skills: [],
      languages: [],
      education: [],
      experiences: [],
      projects: [],
    },
  };
}

/** The write request of a draft: only write keys, and only provided collections. */
export function toWriteRequest(draft: ProfileDraft): Record<string, unknown> {
  const request: Record<string, unknown> = {
    fullName: draft.fullName,
    email: draft.email,
    summary: draft.summary,
  };
  for (const spec of collectionSections) {
    const rows = draft.collections[spec.key];
    if (rows !== undefined) {
      request[spec.key] = rows.map((row) => writeRow(row, spec));
    }
  }
  return request;
}

function writeRow(row: Row, spec: CollectionSpec): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of spec.writeKeys) {
    if (key === "technologies") {
      if (Array.isArray(row.technologies)) {
        out.technologies = (row.technologies as Row[]).map((technology) =>
          writeTechnology(technology),
        );
      }
    } else if (row[key] !== undefined) {
      out[key] = row[key];
    }
  }
  return out;
}

function writeTechnology(technology: Row): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of technologyWriteKeys) {
    if (technology[key] !== undefined) {
      out[key] = technology[key];
    }
  }
  return out;
}

/** True when the profile draft differs from its baseline (the write request is the comparison). */
export function isProfileDirty(draft: ProfileDraft, baseline: ProfileDraft): boolean {
  return JSON.stringify(toWriteRequest(draft)) !== JSON.stringify(toWriteRequest(baseline));
}

/** The personal draft of an envelope (null values become empty text). */
export function personalFromEnvelope(envelope: PersonalDetailsEnvelopeDto): PersonalDraft {
  const details = envelope.personalDetails;
  return {
    dateOfBirth: details?.dateOfBirth ?? "",
    nationality: details?.nationality ?? "",
  };
}

/** A blank personal draft (a profile without personal details yet). */
export const blankPersonal: PersonalDraft = { dateOfBirth: "", nationality: "" };

/** The personal update request: empty text is sent as null. */
export function personalWriteRequest(draft: PersonalDraft): Record<string, string | null> {
  return {
    dateOfBirth: draft.dateOfBirth === "" ? null : draft.dateOfBirth,
    nationality: draft.nationality === "" ? null : draft.nationality,
  };
}

/** True when the personal draft differs from its baseline. */
export function isPersonalDirty(draft: PersonalDraft, baseline: PersonalDraft): boolean {
  return draft.dateOfBirth !== baseline.dateOfBirth || draft.nationality !== baseline.nationality;
}

/** The next display order for an append: one past the largest existing order (default -1). */
export function nextDisplayOrder(rows: readonly Row[]): number | null {
  let largest = -1;
  for (const row of rows) {
    const order = row.displayOrder;
    if (typeof order === "number" && Number.isInteger(order) && order > largest) {
      largest = order;
    }
  }
  return largest >= MAX_DISPLAY_ORDER ? null : largest + 1;
}

/** Appends an empty row to a collection, creating the collection when it is not provided yet. */
export function addRow(draft: ProfileDraft, key: CollectionKey): EditResult {
  const spec = collectionSections.find((section) => section.key === key);
  if (!spec) {
    return { ok: true, draft };
  }
  const rows = draft.collections[key] ?? [];
  const row: Row = {};
  if (spec.ordered) {
    const order = nextDisplayOrder(rows);
    if (order === null) {
      return { ok: false, messageId: "profile.displayOrder.limit" };
    }
    row.displayOrder = order;
  }
  // The presenter's new rows carry these keys as null (`_new_collection_item`); Rust derives the
  // normalized value on save, so null and absent are the same to it.
  if (key === "skills") {
    row.normalizedSkill = null;
    row.category = null;
  } else if (key === "languages") {
    row.normalizedLanguage = null;
  } else if (key === "projects") {
    row.technologies = [];
  }
  return { ok: true, draft: withCollection(draft, key, [...rows, row]) };
}

/** Removes one row from a collection. */
export function removeRow(draft: ProfileDraft, key: CollectionKey, index: number): ProfileDraft {
  const rows = draft.collections[key];
  if (!rows || index < 0 || index >= rows.length) {
    return draft;
  }
  return withCollection(
    draft,
    key,
    rows.filter((_, position) => position !== index),
  );
}

/**
 * The normalized key that an edit to a name field makes stale.
 * Editing the name clears it so the core derives it again on save.
 */
const normalizedKeyOf: Partial<Record<string, string>> = {
  "skills:skill": "normalizedSkill",
  "languages:language": "normalizedLanguage",
};

/** Sets one editable field of a row. Cleared dates store null; other fields keep the typed text. */
export function setRowField(
  draft: ProfileDraft,
  key: CollectionKey,
  index: number,
  field: string,
  value: string,
): ProfileDraft {
  const spec = collectionSections.find((section) => section.key === key);
  const rows = draft.collections[key];
  const fieldSpec = spec?.fields.find((item) => item.key === field);
  if (!rows || !fieldSpec || index < 0 || index >= rows.length) {
    return draft;
  }
  const stored = fieldSpec.kind === "date" && value === "" ? null : value;
  const staleKey = normalizedKeyOf[`${key}:${field}`];
  return withCollection(
    draft,
    key,
    rows.map((row, position) => {
      if (position !== index) {
        return row;
      }
      const next: Row = { ...row, [field]: stored };
      if (staleKey !== undefined) {
        // The presenter sets the stale normalized key to null (`set_collection_field`).
        next[staleKey] = null;
      }
      return next;
    }),
  );
}

/** Appends a technology under one project. */
export function addTechnology(draft: ProfileDraft, projectIndex: number): EditResult {
  const rows = draft.collections.projects;
  const project = rows?.[projectIndex];
  if (!rows || !project) {
    return { ok: true, draft };
  }
  const technologies = Array.isArray(project.technologies) ? (project.technologies as Row[]) : [];
  const order = nextDisplayOrder(technologies);
  if (order === null) {
    return { ok: false, messageId: "profile.displayOrder.limit" };
  }
  return {
    ok: true,
    draft: replaceProject(draft, projectIndex, {
      ...project,
      technologies: [...technologies, { normalizedTechnology: null, displayOrder: order }],
    }),
  };
}

/** Removes one technology of a project. */
export function removeTechnology(
  draft: ProfileDraft,
  projectIndex: number,
  technologyIndex: number,
): ProfileDraft {
  const project = draft.collections.projects?.[projectIndex];
  if (!project || !Array.isArray(project.technologies)) {
    return draft;
  }
  const technologies = (project.technologies as Row[]).filter(
    (_, position) => position !== technologyIndex,
  );
  return replaceProject(draft, projectIndex, { ...project, technologies });
}

/** Sets the technology name of one row under a project. */
export function setTechnology(
  draft: ProfileDraft,
  projectIndex: number,
  technologyIndex: number,
  value: string,
): ProfileDraft {
  const project = draft.collections.projects?.[projectIndex];
  if (!project || !Array.isArray(project.technologies)) {
    return draft;
  }
  const technologies = (project.technologies as Row[]).map((technology, position) => {
    if (position !== technologyIndex) {
      return technology;
    }
    // Editing the name clears the stored normalized key so the core derives it again.
    return { ...technology, technology: value, normalizedTechnology: null };
  });
  return replaceProject(draft, projectIndex, { ...project, technologies });
}

function withCollection(draft: ProfileDraft, key: CollectionKey, rows: Row[]): ProfileDraft {
  return { ...draft, collections: { ...draft.collections, [key]: rows } };
}

function replaceProject(draft: ProfileDraft, index: number, project: Row): ProfileDraft {
  const rows = draft.collections.projects ?? [];
  return withCollection(
    draft,
    "projects",
    rows.map((row, position) => (position === index ? project : row)),
  );
}

/** The project fields that differ from the baseline, in the Rust check order. */
export function changedProjectFields(draft: Row, baseline: Row): string[] {
  return projectFields.filter((field) => draft[field] !== baseline[field]);
}

/** True when the project technologies differ from the baseline (they save with the profile). */
function technologiesDiffer(draft: Row, baseline: Row): boolean {
  return (
    JSON.stringify(draft.technologies ?? null) !== JSON.stringify(baseline.technologies ?? null)
  );
}

/**
 * The saved row of a draft project row, matched by id (never by position: removing a row shifts the
 * draft indexes but not the baseline's). Undefined for a row with no id or no saved match.
 */
export function baselineProject(baseline: ProfileDraft | null, draftRow: Row | undefined) {
  const id = draftRow?.id;
  if (typeof id !== "string") {
    return undefined;
  }
  return baseline?.collections.projects?.find((row) => row.id === id);
}

/**
 * The UI mirror of the project-save rule. Rust decides on save.
 * Only the first changed field is checked; url and description cannot be cleared.
 */
export function canSaveProject(
  draft: ProfileDraft | null,
  baseline: ProfileDraft | null,
  index: number,
): boolean {
  const current = draft?.collections.projects?.[index];
  const saved = baselineProject(baseline, current);
  if (!current || !saved) {
    return false;
  }
  if (technologiesDiffer(current, saved)) {
    return false;
  }
  const [first] = changedProjectFields(current, saved);
  if (first === undefined) {
    return false;
  }
  if (first === "displayOrder") {
    return true;
  }
  const value = current[first];
  return typeof value === "string" && value.trim() !== "";
}

/** The project-only patch: the changed fields, the project id and the revision. */
export function projectPatchRequest(
  profileId: string,
  draft: ProfileDraft,
  baseline: ProfileDraft,
  index: number,
  revision: string,
): Record<string, unknown> {
  const current = draft.collections.projects?.[index] ?? {};
  const saved = baselineProject(baseline, current) ?? {};
  const request: Record<string, unknown> = {
    profileId,
    projectId: current.id,
    expectedRevision: revision,
  };
  for (const field of changedProjectFields(current, saved)) {
    request[field] = current[field];
  }
  return request;
}

/** True when a project row differs from the baseline in one of the project-only fields. */
export function projectHasChanges(
  draft: ProfileDraft | null,
  baseline: ProfileDraft | null,
  index: number,
): boolean {
  const current = draft?.collections.projects?.[index];
  const saved = baselineProject(baseline, current);
  return Boolean(current && saved && changedProjectFields(current, saved).length > 0);
}
