import { describe, expect, it } from "vitest";
import {
  exportDoneMessage,
  exportFailureMessage,
  isIdUsable,
  messageText,
  previewFailureMessage,
  summaryText,
} from "./artifact";

describe("artifact helpers", () => {
  it("accepts a non-blank ID of at most 36 characters", () => {
    expect(isIdUsable("")).toBe(false);
    expect(isIdUsable("   ")).toBe(false);
    expect(isIdUsable("00000000-0000-4000-8000-0000000000d1")).toBe(true);
    expect(isIdUsable("x".repeat(37))).toBe(false);
  });

  it("formats the summary with the one-based copy and no path", () => {
    const text = summaryText({ displayName: "cv.pdf", pageCount: 3, byteSize: "9000" });
    expect(text).toBe("cv.pdf · 3 pages · 9000 bytes");
  });

  it("shows the done copy with the file name, and nothing after a cancel", () => {
    expect(exportDoneMessage(null)).toBeNull();
    const done = exportDoneMessage({ exportedName: "copy.pdf" });
    expect(done).not.toBeNull();
    expect(messageText(done ?? { id: "artifacts.close" })).toBe("PDF exported to copy.pdf.");
  });

  it("maps export failures to the refusal or the uncertainty copy", () => {
    expect(
      exportFailureMessage({ code: "export_failed", messageId: "artifacts.export.failed" }),
    ).toEqual({
      id: "artifacts.export.failed",
    });
    expect(
      exportFailureMessage({ code: "export_uncertain", messageId: "artifacts.export.uncertain" }),
    ).toEqual({ id: "artifacts.export.uncertain" });
    expect(exportFailureMessage({ code: "busy", messageId: "artifacts.busy" })).toBeNull();
    // A failure that is not a typed command error reads as the generic export refusal.
    expect(exportFailureMessage(new Error("raw text"))).toEqual({ id: "artifacts.export.failed" });
  });

  it("maps preview failures to the viewer copy and shows nothing for the busy refusal", () => {
    expect(
      previewFailureMessage({ code: "preview_failed", messageId: "artifacts.preview.failed" }),
    ).toEqual({
      id: "artifacts.preview.failed",
    });
    expect(
      previewFailureMessage({ code: "validation_error", messageId: "artifacts.input.id" }),
    ).toEqual({
      id: "artifacts.input.id",
    });
    expect(previewFailureMessage({ code: "busy", messageId: "artifacts.busy" })).toBeNull();
  });
});
