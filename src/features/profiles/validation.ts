import { formatMessage, type MessageId } from "../../i18n/messages";
import type { PersonalDraft, ProfileDraft, Row } from "./draft";
import { profileGuides } from "./guide";
import { type CollectionSpec, collectionSections } from "./schema";

/**
 * UX pre-validation of the profile form: the same rules Rust applies on save (see
 * `crates/core/src/profiles/draft.rs` and `values.rs`), so a mistake shows next to its field before
 * the save. Rust stays the validator of record and rejects whatever these checks let through.
 *
 * Errors are keyed by the id of the input they belong to, in form order, so the first key is the
 * first field to fix and `document.getElementById(key)` is the input.
 */
export type FieldErrors = ReadonlyMap<string, FieldError>;

/** A catalogue message and its placeholder values. */
export type FieldError = { id: MessageId; params?: Record<string, string> };

/** The input ids of the fields outside the collections. */
export const FIELD_IDS = {
  fullName: "profile-full-name",
  email: "profile-email",
  dateOfBirth: "personal-date-of-birth",
} as const;

/** The input id of a collection row field (`education-0-startDate`). */
export function rowFieldId(collection: string, index: number, field: string): string {
  return `${collection}-${index}-${field}`;
}

/** The input id of a project technology row. */
export function technologyFieldId(projectIndex: number, technologyIndex: number): string {
  return `project-${projectIndex}-technology-${technologyIndex}`;
}

/** The text of a field: missing and null values are empty. */
function textOf(row: Row, key: string): string {
  const value = row[key];
  return typeof value === "string" || typeof value === "number" ? String(value) : "";
}

const blank = (text: string): boolean => text.trim() === "";

/** Rust `is_basic_email`: one `@`, no whitespace, and a dot inside the domain (not at its ends). */
export function isBasicEmail(text: string): boolean {
  return /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(text);
}

/** A canonical `YYYY-MM-DD` calendar date with a year from 1 to 9999 (Rust `parse_iso_date`). */
export function isIsoDate(text: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  if (match === null) {
    return false;
  }
  const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
  if (year < 1 || month < 1 || month > 12 || day < 1) {
    return false;
  }
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  const lengths = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return day <= (lengths[month - 1] ?? 0);
}

/**
 * The error of an optional date text, or null. Empty is fine (the form stores it as null); anything
 * else must be exactly `YYYY-MM-DD`, spaces included: Rust does not trim a date.
 */
export function dateError(text: string): FieldError | null {
  return text === "" || isIsoDate(text) ? null : { id: "validation.date.invalid" };
}

/** The error of the personal-details draft, keyed by input id. */
export function validatePersonal(personal: PersonalDraft): FieldErrors {
  const errors = new Map<string, FieldError>();
  const date = dateError(personal.dateOfBirth);
  if (date !== null) {
    errors.set(FIELD_IDS.dateOfBirth, date);
  }
  return errors;
}

function requiredError(label: string, example: string): FieldError {
  return { id: "validation.required", params: { label, example } };
}

/** Checks the required text fields, the date shapes and the date order of one collection row. */
function validateRow(
  spec: CollectionSpec,
  row: Row,
  index: number,
  errors: Map<string, FieldError>,
): void {
  for (const field of spec.fields) {
    const text = textOf(row, field.key);
    const id = rowFieldId(spec.key, index, field.key);
    if (field.guide.required && blank(text)) {
      errors.set(id, requiredError(field.label, field.guide.example));
    } else if (field.kind === "date") {
      const date = dateError(text);
      if (date !== null) {
        errors.set(id, date);
      }
    }
  }
  const start = textOf(row, "startDate");
  const end = textOf(row, "endDate");
  const endId = rowFieldId(spec.key, index, "endDate");
  if (isIsoDate(start) && isIsoDate(end) && end < start && !errors.has(endId)) {
    errors.set(endId, { id: "validation.date.order" });
  }
}

/** Contacts must not repeat a type and value (Rust compares them stripped, the type lower-cased). */
function validateContactDuplicates(rows: Row[], errors: Map<string, FieldError>): void {
  const seen = new Set<string>();
  rows.forEach((row, index) => {
    const type = textOf(row, "contactType").trim().toLowerCase();
    const value = textOf(row, "contactValue").trim();
    if (type === "" || value === "") {
      return;
    }
    const key = `${type}\u0000${value}`;
    const id = rowFieldId("contacts", index, "contactValue");
    if (seen.has(key) && !errors.has(id)) {
      errors.set(id, { id: "validation.contact.duplicate" });
    }
    seen.add(key);
  });
}

/** Every error of the profile draft, in form order. Empty when Rust would not refuse the shape. */
export function validateDraft(draft: ProfileDraft): FieldErrors {
  const errors = new Map<string, FieldError>();
  if (blank(draft.fullName)) {
    errors.set(
      FIELD_IDS.fullName,
      requiredError(formatMessage("ui.profile.fullName"), profileGuides.fullName.example),
    );
  }
  if (blank(draft.email)) {
    errors.set(
      FIELD_IDS.email,
      requiredError(formatMessage("ui.profile.email"), profileGuides.email.example),
    );
  } else if (!isBasicEmail(draft.email.trim())) {
    errors.set(FIELD_IDS.email, { id: "validation.email.invalid" });
  }
  for (const spec of collectionSections) {
    const rows = draft.collections[spec.key] ?? [];
    rows.forEach((row, index) => {
      validateRow(spec, row, index, errors);
      if (spec.key === "projects" && Array.isArray(row.technologies)) {
        (row.technologies as Row[]).forEach((technology, technologyIndex) => {
          if (blank(textOf(technology, "technology"))) {
            errors.set(
              technologyFieldId(index, technologyIndex),
              requiredError(formatMessage("ui.technology.label"), profileGuides.technology.example),
            );
          }
        });
      }
    });
    if (spec.key === "contacts") {
      validateContactDuplicates(rows, errors);
    }
  }
  return errors;
}
