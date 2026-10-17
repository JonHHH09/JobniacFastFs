import { describe, expect, it } from "vitest";
import { scoreValue, wholeNumber } from "./reviewDraft";

describe("wholeNumber", () => {
  it.each([
    ["0", 0],
    ["7", 7],
    ["  42  ", 42],
    ["007", 7],
  ])("reads %j as %i", (text, expected) => {
    expect(wholeNumber(text)).toBe(expected);
  });

  it.each(["", "   ", "1e2", "0x10", "5.0", "5.5", "+5", "-5", "-0", "1_000", "٣", "5 6", "abc"])(
    "refuses %j",
    (text) => {
      expect(wholeNumber(text)).toBeNull();
    },
  );
});

describe("scoreValue", () => {
  it.each([
    ["0", 0],
    [" 55 ", 55],
    ["100", 100],
  ])("accepts %j as %i", (text, expected) => {
    expect(scoreValue(text)).toBe(expected);
  });

  it.each(["", "101", "1e2", "0x10", "5.0", "-1", "+5"])("refuses %j", (text) => {
    expect(scoreValue(text)).toBeNull();
  });
});
