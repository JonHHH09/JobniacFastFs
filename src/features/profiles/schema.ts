/**
 * The collection schema of the profile form. The write keys are
 * listed in write order and the editable fields are the visible ones. The Rust
 * use cases drop unknown keys on read and write, so the UI sends only these keys.
 */

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

const endDate: FieldSpec = { key: "endDate", label: "End date", kind: "date" };
const startDate: FieldSpec = { key: "startDate", label: "Start date", kind: "date" };

/** The collections of the profile form, in form order. */
export const collectionSections: readonly CollectionSpec[] = [
  {
    key: "contacts",
    label: "Contacts",
    writeKeys: ["id", "contactType", "contactValue", "label"],
    fields: [
      { key: "contactType", label: "Type" },
      { key: "contactValue", label: "Contact" },
      { key: "label", label: "Label" },
    ],
    ordered: false,
  },
  {
    key: "links",
    label: "Links",
    writeKeys: ["id", "linkType", "url", "label"],
    fields: [
      { key: "linkType", label: "Type" },
      { key: "url", label: "Link" },
      { key: "label", label: "Label" },
    ],
    ordered: false,
  },
  {
    key: "skills",
    label: "Skills",
    writeKeys: ["id", "skill", "normalizedSkill", "category", "displayOrder"],
    fields: [
      { key: "skill", label: "Skill" },
      { key: "category", label: "Category" },
    ],
    ordered: true,
  },
  {
    key: "languages",
    label: "Languages",
    writeKeys: ["id", "language", "normalizedLanguage", "proficiency", "displayOrder"],
    fields: [
      { key: "language", label: "Language" },
      { key: "proficiency", label: "Proficiency" },
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
      { key: "institution", label: "Institution" },
      { key: "degree", label: "Degree" },
      { key: "field", label: "Field of study" },
      { key: "location", label: "Location" },
      startDate,
      endDate,
      { key: "relevantFocus", label: "Relevant focus" },
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
      { key: "company", label: "Company" },
      { key: "title", label: "Role" },
      { key: "location", label: "Location" },
      startDate,
      endDate,
      { key: "description", label: "Description" },
    ],
    ordered: true,
  },
  {
    key: "projects",
    label: "Projects",
    writeKeys: ["id", "name", "url", "description", "displayOrder", "technologies"],
    fields: [
      { key: "name", label: "Project" },
      { key: "url", label: "Link" },
      { key: "description", label: "Description" },
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
