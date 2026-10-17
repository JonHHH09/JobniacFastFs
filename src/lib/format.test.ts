import { describe, expect, it } from "vitest";
import { isMessageId, messages } from "../i18n/messages";
import {
  ENUM_VALUES,
  type EnumKind,
  enumMessageId,
  formatBytes,
  formatInstant,
  formatRelative,
  humanizeEnum,
  sentenceCase,
} from "./format";

describe("humanizeEnum", () => {
  it("labels every listed value of every kind from the catalogue", () => {
    for (const [kind, values] of Object.entries(ENUM_VALUES)) {
      for (const value of values) {
        const id = enumMessageId(kind as EnumKind, value);
        expect(isMessageId(id), `${id} needs copy`).toBe(true);
        const label = humanizeEnum(kind as EnumKind, value);
        expect(label).toBe(messages[id as keyof typeof messages]);
        // A label is words, never the machine form.
        expect(label).not.toMatch(/[A-Z]_[A-Z]/);
      }
    }
  });

  it("has no enum copy that ENUM_VALUES does not list", () => {
    const listed = new Set(
      Object.entries(ENUM_VALUES).flatMap(([kind, values]) =>
        values.map((value) => enumMessageId(kind as EnumKind, value)),
      ),
    );
    const orphans = Object.keys(messages).filter((id) => id.startsWith("enum.") && !listed.has(id));
    expect(orphans).toEqual([]);
  });

  it("reads the labels for outcomes and components", () => {
    expect(humanizeEnum("matchOutcome", "STRONG_MATCH")).toBe("Strong match");
    expect(humanizeEnum("matchComponent", "EXPERIENCE_SENIORITY")).toBe("Experience and seniority");
    expect(humanizeEnum("disagreementStatus", "ACKNOWLEDGED")).toBe("Acknowledged");
    expect(humanizeEnum("disagreementReason", "OVERALL_DELTA")).toBe("Overall scores differ");
  });

  it("falls back to a sentence-cased version of an unknown value", () => {
    expect(humanizeEnum("matchStatus", "SOMETHING_NEW")).toBe("Something new");
    expect(humanizeEnum("generationKind", "someNewKind")).toBe("Some new kind");
    expect(humanizeEnum("disagreementReason", "score_delta_exceeds_threshold")).toBe(
      "Score delta exceeds threshold",
    );
  });

  it("finds a value written in another case", () => {
    expect(humanizeEnum("disagreementStatus", "open")).toBe("Open");
    expect(humanizeEnum("healthStatus", "up")).toBe("Up");
  });

  it("matches job types ignoring case and punctuation", () => {
    expect(humanizeEnum("jobType", "Full-Time")).toBe("Full time");
    expect(humanizeEnum("jobType", "working student")).toBe("Working student");
    expect(humanizeEnum("jobType", "Professional / Experienced")).toBe(
      "Professional or experienced",
    );
    expect(humanizeEnum("jobType", "Seasonal")).toBe("Seasonal");
  });

  it("returns an empty string for an empty value", () => {
    expect(humanizeEnum("matchStatus", "")).toBe("");
    expect(humanizeEnum("matchStatus", "  ")).toBe("");
    expect(humanizeEnum("matchStatus", null)).toBe("");
    expect(humanizeEnum("matchStatus", undefined)).toBe("");
  });

  it("never reads an inherited object member as copy", () => {
    expect(humanizeEnum("matchStatus", "constructor")).toBe("Constructor");
    expect(humanizeEnum("matchStatus", "__proto__")).toBe("Proto");
  });
});

describe("sentenceCase", () => {
  it("handles snake, kebab, camel and upper case", () => {
    expect(sentenceCase("EXPERIENCE_SENIORITY")).toBe("Experience seniority");
    expect(sentenceCase("created-job")).toBe("Created job");
    expect(sentenceCase("reusedExistingJob")).toBe("Reused existing job");
    expect(sentenceCase("  ")).toBe("");
  });
});

describe("formatInstant", () => {
  it("shows date and time in the given locale and zone", () => {
    expect(formatInstant("2026-03-15T10:15:00Z", { locale: "en-US", timeZone: "UTC" })).toBe(
      "Mar 15, 2026, 10:15 AM",
    );
    expect(formatInstant("2026-03-15T10:15:00Z", { locale: "de-DE", timeZone: "UTC" })).toBe(
      "15.03.2026, 10:15",
    );
  });

  it("reads the ISO form with microseconds and an offset", () => {
    expect(
      formatInstant("2026-03-15T10:15:00.123456+00:00", { locale: "en-US", timeZone: "UTC" }),
    ).toBe("Mar 15, 2026, 10:15 AM");
  });

  it("keeps text that is not a date and blanks null", () => {
    expect(formatInstant("not a date")).toBe("not a date");
    expect(formatInstant(null)).toBe("");
    expect(formatInstant(undefined)).toBe("");
    expect(formatInstant("")).toBe("");
  });
});

describe("formatRelative", () => {
  const now = Date.parse("2026-03-15T12:00:00Z");
  it("reads past and future instants", () => {
    expect(formatRelative("2026-03-13T12:00:00Z", now, "en-US")).toBe("2 days ago");
    expect(formatRelative("2026-03-15T09:00:00Z", now, "en-US")).toBe("3 hours ago");
    expect(formatRelative("2026-03-15T12:05:00Z", now, "en-US")).toBe("in 5 minutes");
    expect(formatRelative("2026-03-14T12:00:00Z", now, "en-US")).toBe("yesterday");
    expect(formatRelative("2026-03-15T11:59:40Z", now, "en-US")).toBe("now");
    expect(formatRelative("2024-03-15T12:00:00Z", now, "en-US")).toBe("2 years ago");
  });

  it("is empty for a value that is not a date", () => {
    expect(formatRelative("nope", now)).toBe("");
    expect(formatRelative(null, now)).toBe("");
  });
});

describe("formatBytes", () => {
  it("uses binary units", () => {
    expect(formatBytes(812, "en-US")).toBe("812 B");
    expect(formatBytes(1536, "en-US")).toBe("1.5 KB");
    expect(formatBytes(120 * 1024, "en-US")).toBe("120 KB");
    expect(formatBytes("2097152", "en-US")).toBe("2 MB");
    expect(formatBytes(null)).toBe("");
    expect(formatBytes(-1)).toBe("");
  });
});
