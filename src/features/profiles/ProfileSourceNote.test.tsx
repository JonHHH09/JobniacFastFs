import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SourceViewDto } from "../../ipc/bindings";
import { ProfileSourceNote, sourceIdLines, sourceLines } from "./ProfileSourceNote";

// Synthetic identifiers only.
const PROFILE_ID = "00000000-0000-4000-8000-0000000000b1";
const SOURCE_ID = "00000000-0000-4000-8000-0000000000c1";
const EXTRACTION_ID = "00000000-0000-4000-8000-0000000000e1";

const source: SourceViewDto = {
  sourceLinkId: SOURCE_ID,
  profileId: PROFILE_ID,
  extractionId: EXTRACTION_ID,
  sourceType: "resumePdf",
  createdAt: "2026-01-02T03:04:05Z",
};

function renderNote(profileId: string | null) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ProfileSourceNote profileId={profileId} />
    </QueryClientProvider>,
  );
}

describe("source lines", () => {
  it("names the source type and the creation time", () => {
    const lines = sourceLines(source);
    expect(lines[0]).toBe("Source type: Resume PDF");
    expect(lines[1]).toMatch(/^Created: /);
  });

  it("lists the raw identifiers for the details block", () => {
    expect(sourceIdLines(source)).toEqual([
      `Source link ID: ${SOURCE_ID}`,
      `Profile ID: ${PROFILE_ID}`,
      `Extraction ID: ${EXTRACTION_ID}`,
    ]);
  });
});

describe("ProfileSourceNote", () => {
  it("shows the PDF source of a profile built from a PDF", async () => {
    mockIPC((cmd) => (cmd === "profile_pdf_lookup_source" ? source : undefined));
    renderNote(PROFILE_ID);
    expect(await screen.findByTestId("profileSourceNote")).toHaveTextContent("Source type");
  });

  it("shows nothing for a profile without a PDF source", async () => {
    let asked = false;
    mockIPC((cmd) => {
      if (cmd === "profile_pdf_lookup_source") {
        asked = true;
        throw { code: "not_found", messageId: "profilePdf.error.generic" };
      }
      return undefined;
    });
    renderNote(PROFILE_ID);
    await waitFor(() => expect(asked).toBe(true));
    expect(screen.queryByTestId("profileSourceNote")).not.toBeInTheDocument();
  });

  it("does not look anything up without a saved profile", () => {
    let asked = false;
    mockIPC(() => {
      asked = true;
    });
    renderNote(null);
    expect(asked).toBe(false);
    expect(screen.queryByTestId("profileSourceNote")).not.toBeInTheDocument();
  });
});
