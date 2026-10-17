import { describe, expect, it } from "vitest";
import { confirmationMessage, deleteFailureId, isBusyRefusal, isNotFound } from "./confirmation";

const PROFILE_TAIL =
  "This permanently removes the profile and its dependent resumes, cover letters, and matching reports and reviews, including its links to imported documents. Imported documents are not deleted. Generated files owned by Jobniac that are no longer referenced are scheduled for cleanup. This cannot be undone.";

describe("delete confirmation copy", () => {
  it("names a profile in double quotes and says what is removed", () => {
    expect(confirmationMessage("profile", "Ada Example", false)).toBe(
      `Delete profile "Ada Example"? ${PROFILE_TAIL}`,
    );
  });

  it("falls back to 'this profile' for an empty label, after trimming", () => {
    expect(confirmationMessage("profile", "   ", false)).toBe(
      `Delete profile this profile? ${PROFILE_TAIL}`,
    );
  });

  it("trims the label before it is quoted", () => {
    expect(confirmationMessage("profile", "  Ada Example  ", false)).toContain(
      'Delete profile "Ada Example"?',
    );
  });

  it("appends the discard notice for unsaved edits, per kind", () => {
    expect(
      confirmationMessage("profile", "Ada", true).endsWith(
        " Your unsaved edits to this profile will be discarded.",
      ),
    ).toBe(true);
    expect(
      confirmationMessage("job", "Synthetic Engineer", true).endsWith(
        " Your unsaved edits to this job will be discarded.",
      ),
    ).toBe(true);
  });

  it("uses the job wording with the job subject", () => {
    const message = confirmationMessage("job", "Synthetic Engineer", false);
    expect(
      message.startsWith(
        'Delete job "Synthetic Engineer"? This permanently removes the job and its dependent resumes',
      ),
    ).toBe(true);
    expect(confirmationMessage("job", "", false).startsWith("Delete job this job? ")).toBe(true);
  });

  it("keeps placeholder-like text in the label verbatim", () => {
    expect(confirmationMessage("profile", "{subject}", false)).toContain('"{subject}"');
  });
});

describe("delete failure copy", () => {
  it("maps a generation in progress to its own copy", () => {
    expect(deleteFailureId("profile", { code: "conflict", messageId: "x" })).toBe(
      "delete.failed.conflict",
    );
    expect(deleteFailureId("job", { code: "conflict", messageId: "x" })).toBe(
      "delete.failed.conflict",
    );
  });

  it("maps every other code, and a non-command error, to the fixed kind copy", () => {
    expect(deleteFailureId("profile", { code: "internal_error", messageId: "" })).toBe(
      "delete.failed.profile",
    );
    expect(deleteFailureId("job", { code: "validation_error", messageId: "" })).toBe(
      "delete.failed.job",
    );
    expect(deleteFailureId("profile", "raw text")).toBe("delete.failed.profile");
  });

  it("shows no copy for a refused call, because nothing ran", () => {
    expect(deleteFailureId("job", { code: "busy", messageId: "job.busy" })).toBeNull();
  });

  it("recognizes a busy refusal only", () => {
    expect(isBusyRefusal({ code: "busy", messageId: "job.busy" })).toBe(true);
    expect(isBusyRefusal({ code: "conflict", messageId: "" })).toBe(false);
    expect(isBusyRefusal("raw text")).toBe(false);
  });

  it("recognizes a missing job", () => {
    expect(isNotFound({ code: "not_found", messageId: "" })).toBe(true);
    expect(isNotFound({ code: "conflict", messageId: "" })).toBe(false);
  });
});
