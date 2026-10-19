import { formatMessage, type MessageId } from "../../i18n/messages";

/**
 * What the profile form tells the operator about one input: whether it may be left empty, the kind
 * of value it accepts and an example of it. The text sits under the field; the checks that back it
 * are in `validation.ts`.
 */
export type FieldGuide = {
  readonly required: boolean;
  readonly type: "text" | "longText" | "email" | "date" | "link";
  /** A realistic, synthetic value. Also the example inside the "is required" error. */
  readonly example: string;
  /** One extra sentence, for rules the type does not say (for example "leave empty if ongoing"). */
  readonly note?: string;
};

const TYPE_LABEL: Record<FieldGuide["type"], MessageId> = {
  text: "guide.type.text",
  longText: "guide.type.longText",
  email: "guide.type.email",
  date: "guide.type.date",
  link: "guide.type.link",
};

/** The hint line under a field: "Required · Text · Example: Ada Example", plus the note. */
export function guideText(guide: FieldGuide): string {
  const parts = [
    formatMessage(guide.required ? "guide.required" : "guide.optional"),
    formatMessage(TYPE_LABEL[guide.type]),
    formatMessage("guide.example", { example: guide.example }),
  ];
  const line = parts.join(" · ");
  return guide.note === undefined ? line : `${line}. ${guide.note}`;
}

/** The guides of the fields outside the collections. */
export const profileGuides = {
  fullName: { required: true, type: "text", example: "Ada Example" },
  email: { required: true, type: "email", example: "ada@example.com" },
  summary: {
    required: false,
    type: "longText",
    example: "Backend engineer with six years of experience in Rust and PostgreSQL",
  },
  dateOfBirth: {
    required: false,
    type: "date",
    example: "1990-04-23",
    note: "Leave empty to clear it",
  },
  nationality: {
    required: false,
    type: "text",
    example: "German",
    note: "Leave empty to clear it",
  },
  technology: { required: true, type: "text", example: "PostgreSQL" },
} as const satisfies Record<string, FieldGuide>;
