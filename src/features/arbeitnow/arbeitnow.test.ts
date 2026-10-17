import { describe, expect, it } from "vitest";
import type { ArbeitnowPageDto, ArbeitnowRowDto } from "../../ipc/bindings";
import {
  type ArbeitnowView,
  buildScanRequest,
  canImportRow,
  EMPTY_FORM,
  failOperation,
  failureMessage,
  finishImport,
  finishPage,
  formatPostedDate,
  INITIAL_VIEW,
  importButtonMessage,
  rejectionMessageId,
  startOperation,
  statusMessage,
} from "./arbeitnow";

const ROW: ArbeitnowRowDto = {
  rowId: "row-id-0000000000000000000000000000000a",
  title: "Synthetic Engineer",
  company: "Example Labs",
  location: "Berlin",
  remote: true,
  postedAt: "2026-01-02",
  excerpt: "",
  tags: [],
  jobTypes: [],
  importStatus: "none",
};

const PAGE: ArbeitnowPageDto = { rows: [ROW], hasNextPage: true, consistencyNotice: "Notice." };

function viewWithRow(): ArbeitnowView {
  return finishPage(INITIAL_VIEW, "scan", PAGE);
}

describe("buildScanRequest", () => {
  it("sends blank query and location as null and the text otherwise as typed", () => {
    const request = buildScanRequest({
      ...EMPTY_FORM,
      query: "   ",
      location: " Berlin ",
      tags: "rust",
      jobTypes: "",
      remoteOnly: true,
    });
    expect(request).toEqual({
      query: null,
      location: " Berlin ",
      remoteOnly: true,
      visaSponsorship: false,
      tags: "rust",
      jobTypes: "",
    });
  });

  it("sends the flags as booleans", () => {
    const request = buildScanRequest({ ...EMPTY_FORM, visaSponsorship: true });
    expect(request.visaSponsorship).toBe(true);
    expect(request.remoteOnly).toBe(false);
  });
});

describe("formatPostedDate", () => {
  it("shows the UTC posting date in the locale's medium format", () => {
    expect(formatPostedDate("2026-01-02", "en-US")).toBe("Jan 2, 2026");
  });

  it("returns text that is not a date as it is and nothing for null", () => {
    expect(formatPostedDate("soon", "en-US")).toBe("soon");
    expect(formatPostedDate(null, "en-US")).toBe("");
  });
});

describe("import copy and gate", () => {
  it("maps each import status to its button copy", () => {
    expect(importButtonMessage("none")).toBe("arbeitnow.button.import");
    expect(importButtonMessage("createdJob")).toBe("arbeitnow.button.imported");
    expect(importButtonMessage("reusedExistingJob")).toBe("arbeitnow.button.alreadySaved");
  });

  it("allows an import only when idle and the row is not imported", () => {
    const view = viewWithRow();
    expect(canImportRow(view, view.rows[0])).toBe(true);
    expect(canImportRow(startOperation(view, "scan"), view.rows[0])).toBe(false);
    const imported = finishImport(view, {
      rowId: ROW.rowId,
      status: "createdJob",
      jobId: "00000000-0000-4000-8000-0000000000d1",
    });
    expect(canImportRow(imported, imported.rows[0])).toBe(false);
  });
});

describe("state transitions", () => {
  it("a scan replaces the rows and a next page appends them", () => {
    const first = viewWithRow();
    const second = finishPage(first, "next", {
      rows: [{ ...ROW, rowId: "row-id-0000000000000000000000000000000b" }],
      hasNextPage: false,
      consistencyNotice: "Notice.",
    });
    expect(second.rows.map((row) => row.rowId)).toEqual([
      ROW.rowId,
      "row-id-0000000000000000000000000000000b",
    ]);
    expect(second.hasNextPage).toBe(false);
    const replaced = finishPage(second, "scan", PAGE);
    expect(replaced.rows).toHaveLength(1);
  });

  it("an empty scan reports the empty copy; an empty next page keeps the rows", () => {
    expect(finishPage(INITIAL_VIEW, "scan", { ...PAGE, rows: [] }).message).toBe(
      "arbeitnow.result.empty",
    );
    expect(finishPage(viewWithRow(), "next", { ...PAGE, rows: [] }).message).toBeNull();
  });

  it("a failure keeps the rows, their status and the next-page availability", () => {
    const imported = finishImport(viewWithRow(), {
      rowId: ROW.rowId,
      status: "reusedExistingJob",
      jobId: "00000000-0000-4000-8000-0000000000d1",
    });
    const failed = failOperation(imported, "next", "arbeitnow.error.next.validation");
    expect(failed.rows).toEqual(imported.rows);
    expect(failed.hasNextPage).toBe(true);
    expect(failed.busy).toBeNull();
    expect(statusMessage(failed)).toBe("arbeitnow.error.next.validation");
  });

  it("the progress copy replaces the settled message while busy", () => {
    expect(statusMessage(startOperation(INITIAL_VIEW, "scan"))).toBe("arbeitnow.progress.scan");
    expect(statusMessage(startOperation(INITIAL_VIEW, "next"))).toBe("arbeitnow.progress.next");
    expect(statusMessage(startOperation(INITIAL_VIEW, "import"))).toBe("arbeitnow.progress.import");
  });

  it("a busy refusal sets no copy", () => {
    const failed = failOperation(startOperation(INITIAL_VIEW, "scan"), "scan", "arbeitnow.busy");
    expect(failed.busy).toBeNull();
    expect(statusMessage(failed)).toBeNull();
  });
});

describe("failureMessage", () => {
  it("keeps an Arbeitnow error copy id and falls back to the generic copy otherwise", () => {
    expect(failureMessage("scan", "arbeitnow.error.scan.rateLimited")).toBe(
      "arbeitnow.error.scan.rateLimited",
    );
    expect(failureMessage("import", "arbeitnow.error.import.validation")).toBe(
      "arbeitnow.error.import.validation",
    );
    expect(failureMessage("scan", "")).toBe("arbeitnow.error.scan.generic");
    expect(failureMessage("next", "profiles.error")).toBe("arbeitnow.error.scan.generic");
    expect(failureMessage("import", "arbeitnow.close")).toBe("arbeitnow.error.import.generic");
  });
});

describe("rejectionMessageId", () => {
  it("reads the message id of a typed rejection and nothing else", () => {
    expect(
      rejectionMessageId({
        code: "validation_error",
        messageId: "arbeitnow.error.import.validation",
      }),
    ).toBe("arbeitnow.error.import.validation");
    expect(rejectionMessageId(new Error("raw text with a token"))).toBe("");
    expect(rejectionMessageId("raw string")).toBe("");
    expect(rejectionMessageId({ messageId: 3 })).toBe("");
  });
});
