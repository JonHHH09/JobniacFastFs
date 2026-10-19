import { describe, expect, it } from "vitest";
import { blankDraft, type ProfileDraft } from "./draft";
import { guideText } from "./guide";
import { collectionSections } from "./schema";
import {
  dateError,
  FIELD_IDS,
  isBasicEmail,
  isIsoDate,
  rowFieldId,
  technologyFieldId,
  validateDraft,
  validatePersonal,
} from "./validation";

// Synthetic data only.
function draftWith(
  collections: ProfileDraft["collections"],
  fields: Partial<Pick<ProfileDraft, "fullName" | "email">> = {},
): ProfileDraft {
  return {
    ...blankDraft(),
    fullName: "Ada Example",
    email: "ada@example.com",
    ...fields,
    collections: { ...blankDraft().collections, ...collections },
  };
}

describe("isBasicEmail (Rust is_basic_email)", () => {
  it.each(["ada@example.com", "a@b.c", "first.last+tag@mail.example.org"])("accepts %s", (text) => {
    expect(isBasicEmail(text)).toBe(true);
  });

  it.each(["", "ada", "ada@example", "@example.com", "ada@", "a b@example.com", "a@@b.com"])(
    "refuses %j",
    (text) => {
      expect(isBasicEmail(text)).toBe(false);
    },
  );
});

describe("isIsoDate (Rust parse_iso_date)", () => {
  it.each(["2023-06-30", "2024-02-29", "0001-01-01", "9999-12-31"])("accepts %s", (text) => {
    expect(isIsoDate(text)).toBe(true);
  });

  it.each([
    "2023-6-30",
    "30-06-2023",
    "2023/06/30",
    "2023-02-29",
    "2023-13-01",
    "2023-00-10",
    "0000-01-01",
    " 2023-06-30",
    "2023-06-30 ",
    "June 2023",
  ])("refuses %j", (text) => {
    expect(isIsoDate(text)).toBe(false);
  });

  it("treats an empty optional date as valid but a blank one as invalid", () => {
    expect(dateError("")).toBeNull();
    expect(dateError(" ")).toEqual({ id: "validation.date.invalid" });
  });
});

describe("validateDraft", () => {
  it("accepts a complete draft", () => {
    expect(validateDraft(draftWith({})).size).toBe(0);
  });

  it("asks for the name and the email with an example, keyed by input id", () => {
    const errors = validateDraft(draftWith({}, { fullName: " ", email: "" }));
    expect([...errors.keys()]).toEqual([FIELD_IDS.fullName, FIELD_IDS.email]);
    expect(errors.get(FIELD_IDS.email)).toEqual({
      id: "validation.required",
      params: { label: "Email address", example: "ada@example.com" },
    });
  });

  it("explains a malformed email", () => {
    const errors = validateDraft(draftWith({}, { email: "ada@example" }));
    expect(errors.get(FIELD_IDS.email)).toEqual({ id: "validation.email.invalid" });
  });

  it("requires the required collection fields and not the optional ones", () => {
    const errors = validateDraft(
      draftWith({ skills: [{ skill: "", category: "" }], education: [{ institution: "" }] }),
    );
    expect([...errors.keys()]).toEqual([rowFieldId("skills", 0, "skill")]);
  });

  it("checks date shapes and keeps the first error of a field", () => {
    const errors = validateDraft(
      draftWith({ experiences: [{ startDate: "2020-1-5", endDate: "2019-12-31" }] }),
    );
    expect(errors.get(rowFieldId("experiences", 0, "startDate"))).toEqual({
      id: "validation.date.invalid",
    });
    expect(errors.has(rowFieldId("experiences", 0, "endDate"))).toBe(false);
  });

  it("refuses an end date before the start date, and accepts the same day or no end", () => {
    const before = validateDraft(
      draftWith({ education: [{ startDate: "2020-01-15", endDate: "2019-12-31" }] }),
    );
    expect(before.get(rowFieldId("education", 0, "endDate"))).toEqual({
      id: "validation.date.order",
    });
    const same = validateDraft(
      draftWith({ education: [{ startDate: "2020-01-15", endDate: "2020-01-15" }] }),
    );
    const open = validateDraft(
      draftWith({ education: [{ startDate: "2020-01-15", endDate: null }] }),
    );
    expect(same.size + open.size).toBe(0);
  });

  it("flags a repeated contact type and value on the later entry only", () => {
    const errors = validateDraft(
      draftWith({
        contacts: [
          { contactType: "Email", contactValue: "ada@example.com" },
          { contactType: "email", contactValue: "ada@example.com" },
        ],
      }),
    );
    expect([...errors.keys()]).toEqual([rowFieldId("contacts", 1, "contactValue")]);
  });

  it("requires a technology name under a project", () => {
    const errors = validateDraft(
      draftWith({ projects: [{ name: "Parser", technologies: [{ technology: "Rust" }, {}] }] }),
    );
    expect([...errors.keys()]).toEqual([technologyFieldId(0, 1)]);
  });
});

describe("validatePersonal", () => {
  it("accepts empty and a real date, and refuses another shape", () => {
    expect(validatePersonal({ dateOfBirth: "", nationality: "" }).size).toBe(0);
    expect(validatePersonal({ dateOfBirth: "1990-04-23", nationality: "" }).size).toBe(0);
    expect(
      validatePersonal({ dateOfBirth: "23.04.1990", nationality: "" }).has(FIELD_IDS.dateOfBirth),
    ).toBe(true);
  });
});

describe("field guides", () => {
  it("gives every collection field a guide with an example", () => {
    for (const spec of collectionSections) {
      for (const field of spec.fields) {
        expect(field.guide.example, `${spec.key}.${field.key}`).not.toBe("");
      }
    }
  });

  it("marks date fields as dates and every date example as a valid date", () => {
    for (const spec of collectionSections) {
      for (const field of spec.fields.filter((item) => item.kind === "date")) {
        expect(field.guide.type).toBe("date");
        expect(isIsoDate(field.guide.example)).toBe(true);
      }
    }
  });

  it("composes the hint line from requirement, type and example", () => {
    expect(guideText({ required: true, type: "email", example: "ada@example.com" })).toBe(
      "Required · Email address · Example: ada@example.com",
    );
    expect(
      guideText({ required: false, type: "date", example: "2023-06-30", note: "Leave empty" }),
    ).toBe("Optional · Date as YYYY-MM-DD · Example: 2023-06-30. Leave empty");
  });
});
