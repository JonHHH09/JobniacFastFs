/**
 * The collection schema of the profile form. The write keys are
 * listed in write order and the editable fields are the visible ones. The Rust
 * use cases drop unknown keys on read and write, so the UI sends only these keys.
 */

import type { FieldGuide } from "./guide";

/** A collection of the profile graph that the form edits. */
export type CollectionKey =
  | "contacts"
  | "links"
  | "skills"
  | "languages"
  | "education"
  | "experiences"
  | "projects";

/** One editable text field of a collection row. */
export type FieldSpec = {
  readonly key: string;
  readonly label: string;
  /** Dates store `null` when cleared; every other field stores the typed text. */
  readonly kind?: "date";
  /** What the field accepts, shown under it: required or optional, the kind of value, an example. */
  readonly guide: FieldGuide;
};

/** One collection of the form. */
export type CollectionSpec = {
  readonly key: CollectionKey;
  /** The section title and the add-button label suffix (`Add <label>`). */
  readonly label: string;
  /** Write keys in the order the Rust side expects. */
  readonly writeKeys: readonly string[];
  /** Editable fields, in the order the form shows them. */
  readonly fields: readonly FieldSpec[];
  /** True when rows carry a `displayOrder` that the UI computes for new rows. */
  readonly ordered: boolean;
};

const endDate: FieldSpec = {
  key: "endDate",
  label: "End date",
  kind: "date",
  guide: {
    required: false,
    type: "date",
    example: "2023-06-30",
    note: "Leave empty if it is ongoing",
  },
};
const startDate: FieldSpec = {
  key: "startDate",
  label: "Start date",
  kind: "date",
  guide: { required: false, type: "date", example: "2020-01-15" },
};

/** The collections of the profile form, in form order. */
export const collectionSections: readonly CollectionSpec[] = [
  {
    key: "contacts",
    label: "Contacts",
    writeKeys: ["id", "contactType", "contactValue", "label"],
    fields: [
      {
        key: "contactType",
        label: "Type",
        guide: {
          required: true,
          type: "text",
          example: "email",
          note: "Pick a suggestion or type your own",
        },
      },
      {
        key: "contactValue",
        label: "Contact",
        guide: {
          required: true,
          type: "text",
          example: "ada@example.com",
          note: "The same type and value cannot be listed twice",
        },
      },
      {
        key: "label",
        label: "Label",
        guide: { required: false, type: "text", example: "Work" },
      },
    ],
    ordered: false,
  },
  {
    key: "links",
    label: "Links",
    writeKeys: ["id", "linkType", "url", "label"],
    fields: [
      {
        key: "linkType",
        label: "Type",
        guide: {
          required: true,
          type: "text",
          example: "github",
          note: "Pick a suggestion or type your own",
        },
      },
      {
        key: "url",
        label: "Link",
        guide: {
          required: true,
          type: "link",
          example: "github.com/ada-example",
          note: "https:// is added when missing",
        },
      },
      {
        key: "label",
        label: "Label",
        guide: { required: false, type: "text", example: "Open-source work" },
      },
    ],
    ordered: false,
  },
  {
    key: "skills",
    label: "Skills",
    writeKeys: ["id", "skill", "normalizedSkill", "category", "displayOrder"],
    fields: [
      { key: "skill", label: "Skill", guide: { required: true, type: "text", example: "Rust" } },
      {
        key: "category",
        label: "Category",
        guide: { required: false, type: "text", example: "Programming languages" },
      },
    ],
    ordered: true,
  },
  {
    key: "languages",
    label: "Languages",
    writeKeys: ["id", "language", "normalizedLanguage", "proficiency", "displayOrder"],
    fields: [
      {
        key: "language",
        label: "Language",
        guide: { required: true, type: "text", example: "German" },
      },
      {
        key: "proficiency",
        label: "Proficiency",
        guide: { required: false, type: "text", example: "Fluent (C1)" },
      },
    ],
    ordered: true,
  },
  {
    key: "education",
    label: "Education",
    writeKeys: [
      "id",
      "institution",
      "degree",
      "field",
      "location",
      "startDate",
      "endDate",
      "relevantFocus",
    ],
    fields: [
      {
        key: "institution",
        label: "Institution",
        guide: { required: false, type: "text", example: "Example University" },
      },
      {
        key: "degree",
        label: "Degree",
        guide: { required: false, type: "text", example: "BSc" },
      },
      {
        key: "field",
        label: "Field of study",
        guide: { required: false, type: "text", example: "Computer Science" },
      },
      {
        key: "location",
        label: "Location",
        guide: { required: false, type: "text", example: "Berlin, Germany" },
      },
      startDate,
      endDate,
      {
        key: "relevantFocus",
        label: "Relevant focus",
        guide: { required: false, type: "text", example: "Distributed systems" },
      },
    ],
    ordered: false,
  },
  {
    key: "experiences",
    label: "Experience",
    writeKeys: [
      "id",
      "company",
      "title",
      "location",
      "startDate",
      "endDate",
      "description",
      "displayOrder",
    ],
    fields: [
      {
        key: "company",
        label: "Company",
        guide: { required: false, type: "text", example: "Example Labs" },
      },
      {
        key: "title",
        label: "Role",
        guide: { required: false, type: "text", example: "Backend engineer" },
      },
      {
        key: "location",
        label: "Location",
        guide: { required: false, type: "text", example: "Remote" },
      },
      startDate,
      endDate,
      {
        key: "description",
        label: "Description",
        guide: {
          required: false,
          type: "longText",
          example: "Built the matching service and cut query time by 40%",
        },
      },
    ],
    ordered: true,
  },
  {
    key: "projects",
    label: "Projects",
    writeKeys: ["id", "name", "url", "description", "displayOrder", "technologies"],
    fields: [
      {
        key: "name",
        label: "Project",
        guide: { required: false, type: "text", example: "Resume parser" },
      },
      {
        key: "url",
        label: "Link",
        guide: { required: false, type: "link", example: "https://example.com/resume-parser" },
      },
      {
        key: "description",
        label: "Description",
        guide: {
          required: false,
          type: "longText",
          example: "Command-line tool that turns PDF resumes into structured data",
        },
      },
    ],
    ordered: true,
  },
];

/** The write keys of a project technology row (the sub-list under each project). */
export const technologyWriteKeys: readonly string[] = [
  "id",
  "technology",
  "normalizedTechnology",
  "displayOrder",
];

/** The largest display order allowed; appending past it is refused. */
export const MAX_DISPLAY_ORDER = 2147483647;

/** The fields of a project that a project-only save may change (in the Rust check order). */
export const projectFields = ["name", "url", "description", "displayOrder"] as const;
