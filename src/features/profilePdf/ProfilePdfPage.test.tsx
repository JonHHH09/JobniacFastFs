import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { IngestResultDto } from "../../ipc/bindings";
import { pickerKeys } from "../pickers/ProfilePicker";
import { nullIfBlank, ProfilePdfPage, replacementReady, resultLines } from "./ProfilePdfPage";

// Synthetic identifiers and text only.
const DOCUMENT_ID = "00000000-0000-4000-8000-0000000000a1";
const PROFILE_ID = "00000000-0000-4000-8000-0000000000b1";
const SOURCE_ID = "00000000-0000-4000-8000-0000000000c1";
const EXTRACTION_ID = "00000000-0000-4000-8000-0000000000e1";

/** The requests the page sent, in order, with their decoded payloads. */
let sent: Array<{ cmd: string; body: unknown }> = [];

/** The answer of the mount-time capabilities check; a test sets it to `false` for an unconfigured database. */
let profilePdfAvailable = true;

function serve(handler: (cmd: string, body: unknown) => unknown) {
  sent = [];
  mockIPC((cmd, payload) => {
    if (cmd === "profile_pdf_capabilities_get") {
      return { profilePdf: profilePdfAvailable };
    }
    const body =
      payload && typeof payload === "object" && "requestJson" in payload
        ? JSON.parse((payload as { requestJson: string }).requestJson)
        : payload;
    sent.push({ cmd, body });
    return handler(cmd, body);
  });
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The names the pickers show are already known, so they ask Rust for nothing.
  client.setQueryData(pickerKeys.document(DOCUMENT_ID), {
    id: DOCUMENT_ID,
    displayName: "synthetic-cv.pdf",
    mediaType: "application/pdf",
    byteSize: 2048,
  });
  client.setQueryData(pickerKeys.profile(PROFILE_ID), {
    id: PROFILE_ID,
    label: "Synthetic Candidate",
  });
  return render(
    <QueryClientProvider client={client}>
      <ProfilePdfPage />
    </QueryClientProvider>,
  );
}

function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

function createdResult(): IngestResultDto {
  return {
    status: "createdProfile",
    documentId: DOCUMENT_ID,
    profileId: PROFILE_ID,
    extractionId: EXTRACTION_ID,
    sourceLinkId: SOURCE_ID,
    candidate: null,
  };
}

function fillDocumentId(value = DOCUMENT_ID) {
  fireEvent.change(screen.getByLabelText("Stored PDF document ID"), { target: { value } });
}

describe("profile PDF page", () => {
  it("shows the unavailable intro and disables the actions on mount when Rust reports no database", async () => {
    profilePdfAvailable = false;
    try {
      serve(() => undefined);
      renderPage();

      expect(
        await screen.findByText(
          "Profile PDF service is unavailable. Configure the local database before continuing.",
        ),
      ).toBeInTheDocument();
      expect(screen.getByLabelText("Stored PDF document ID")).toBeDisabled();
      expect(sent).toEqual([]);
    } finally {
      profilePdfAvailable = true;
    }
  });

  it("ingests without a replacement and shows the created profile and its IDs", async () => {
    serve((cmd) => {
      if (cmd === "profile_pdf_ingest") {
        return { kind: "result", result: createdResult() };
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    fillDocumentId(`  ${DOCUMENT_ID}  `);
    fireEvent.click(screen.getByRole("button", { name: "Create profile from PDF" }));

    expect(await screen.findByText("Profile created from the stored PDF.")).toBeInTheDocument();
    expect(screen.getByText(`Profile ID: ${PROFILE_ID}`)).toBeInTheDocument();
    expect(sent).toEqual([
      {
        cmd: "profile_pdf_ingest",
        body: {
          documentId: DOCUMENT_ID,
          existingProfileId: null,
          overwriteExistingProfile: false,
          expectedRevision: null,
          maxCharacters: null,
        },
      },
    ]);
  });

  it("reads the stored PDF text for review without keeping it, and drops it when another PDF is picked", async () => {
    serve((cmd) => {
      if (cmd === "document_preview_stored") {
        return {
          document: {
            id: DOCUMENT_ID,
            displayName: "synthetic-cv.pdf",
            mediaType: "application/pdf",
            byteSize: 2048,
          },
          extraction: { text: "Synthetic resume text", truncated: true },
        };
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    expect(screen.getByRole("button", { name: "Preview extracted text" })).toBeDisabled();
    fillDocumentId();
    fireEvent.click(screen.getByRole("button", { name: "Preview extracted text" }));

    expect(await screen.findByLabelText("Untrusted extracted PDF text")).toHaveValue(
      "Synthetic resume text",
    );
    expect(screen.getByText("Preview truncated at the character limit.")).toBeInTheDocument();
    expect(sent).toEqual([
      { cmd: "document_preview_stored", body: { documentId: DOCUMENT_ID, persist: false } },
    ]);

    fillDocumentId("");
    expect(screen.queryByLabelText("Untrusted extracted PDF text")).not.toBeInTheDocument();
  });

  it("stages a replacement, runs it only after the confirmation, and sends the token back", async () => {
    serve((cmd) => {
      if (cmd === "profile_pdf_ingest") {
        return {
          kind: "confirmationRequired",
          token: "staged-1",
          existingProfileId: PROFILE_ID,
          expectedRevision: "7",
          documentId: DOCUMENT_ID,
        };
      }
      if (cmd === "profile_pdf_confirm_replacement") {
        return { ...createdResult(), status: "updatedProfile" };
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    fillDocumentId();
    fireEvent.change(screen.getByLabelText("Existing profile (optional)"), {
      target: { value: PROFILE_ID },
    });
    fireEvent.click(screen.getByLabelText("Request explicit replacement with confirmation"));
    fireEvent.change(screen.getByLabelText("Expected revision (required for replacement)"), {
      target: { value: "7" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Review replacement…" }));

    expect(
      await screen.findByText(
        "Replace the selected profile at revision 7 using the selected stored PDF?",
      ),
    ).toBeInTheDocument();
    expect(sent.map((call) => call.cmd)).toEqual(["profile_pdf_ingest"]);

    fireEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(
      await screen.findByText("Existing profile replaced from the stored PDF."),
    ).toBeInTheDocument();
    expect(sent.map((call) => call.cmd)).toEqual([
      "profile_pdf_ingest",
      "profile_pdf_confirm_replacement",
    ]);
    expect(sent[1]?.body).toEqual({ token: "staged-1" });
  });

  it("cancels a staged replacement, keeps the entered values and sends the token back", async () => {
    serve((cmd) => {
      if (cmd === "profile_pdf_ingest") {
        return {
          kind: "confirmationRequired",
          token: "staged-2",
          existingProfileId: PROFILE_ID,
          expectedRevision: "3",
          documentId: DOCUMENT_ID,
        };
      }
      if (cmd === "profile_pdf_cancel_replacement") {
        return { restoredState: "idle" };
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    fillDocumentId();
    fireEvent.change(screen.getByLabelText("Existing profile (optional)"), {
      target: { value: PROFILE_ID },
    });
    fireEvent.click(screen.getByLabelText("Request explicit replacement with confirmation"));
    fireEvent.change(screen.getByLabelText("Expected revision (required for replacement)"), {
      target: { value: "3" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Review replacement…" }));
    await screen.findByText("Confirm profile replacement");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(
      await screen.findByText("Replacement cancelled. Your entered values are preserved."),
    ).toBeInTheDocument();
    expect(sent[1]).toEqual({ cmd: "profile_pdf_cancel_replacement", body: { token: "staged-2" } });
    expect(screen.getByLabelText("Stored PDF document ID")).toHaveValue(DOCUMENT_ID);
  });

  it("refuses a replacement with the input copy and keeps the values", async () => {
    serve(() => reject("validation_error", "profilePdf.input.revision"));
    renderPage();
    fillDocumentId();
    fireEvent.change(screen.getByLabelText("Expected revision (required for replacement)"), {
      target: { value: "1.5" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create profile from PDF" }));

    expect(
      await screen.findByText("Expected revision must be a supported non-negative integer."),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Expected revision (required for replacement)")).toHaveValue(
      "1.5",
    );
  });

  it("looks up the PDF source of a profile and lists its fields", async () => {
    serve((cmd) => {
      if (cmd === "profile_pdf_lookup_source") {
        return {
          sourceLinkId: SOURCE_ID,
          profileId: PROFILE_ID,
          extractionId: EXTRACTION_ID,
          sourceType: "resumePdf",
          createdAt: "2026-01-02T03:04:05+00:00",
        };
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    fireEvent.change(screen.getByLabelText("Look up the stored PDF source of a profile"), {
      target: { value: PROFILE_ID },
    });
    fireEvent.click(screen.getByRole("button", { name: "Look up profile PDF source" }));

    expect(await screen.findByText("Source type: Resume PDF")).toBeInTheDocument();
    expect(screen.getByText(`Source link ID: ${SOURCE_ID}`)).toBeInTheDocument();
    expect(sent[0]).toEqual({ cmd: "profile_pdf_lookup_source", body: { profileId: PROFILE_ID } });
  });
});

describe("profile PDF helpers", () => {
  it("lists a candidate's matched reasons in label order and keeps the candidate unwritten", () => {
    const lines = resultLines({
      status: "duplicateProfileCandidate",
      documentId: DOCUMENT_ID,
      profileId: null,
      extractionId: EXTRACTION_ID,
      sourceLinkId: null,
      candidate: {
        profileId: PROFILE_ID,
        matchedOn: ["linkedin", "email", "otherLink"],
        alreadyLinked: false,
      },
    }).map((line) => line.id);
    expect(lines).toEqual([
      "profilePdf.status.duplicateProfileCandidate",
      "profilePdf.summary.documentId",
      "profilePdf.summary.candidateId",
      "profilePdf.summary.matchedOn",
      "profilePdf.summary.notCreated",
      "profilePdf.summary.needsConfirmation",
      "profilePdf.summary.extractionId",
    ]);
  });

  it("replaces the status line with the already-linked line and keeps the stored-document line", () => {
    const lines = resultLines({
      status: "duplicateProfileCandidate",
      documentId: DOCUMENT_ID,
      profileId: null,
      extractionId: EXTRACTION_ID,
      sourceLinkId: null,
      candidate: { profileId: PROFILE_ID, matchedOn: ["profileId"], alreadyLinked: true },
    }).map((line) => line.id);
    expect(lines[0]).toBe("profilePdf.summary.alreadyLinked");
    expect(lines).toContain("profilePdf.summary.documentId");
    expect(lines).not.toContain("profilePdf.summary.needsConfirmation");
  });

  it("shows no not-created line for a reused existing source", () => {
    const lines = resultLines({
      status: "reusedExistingSource",
      documentId: DOCUMENT_ID,
      profileId: PROFILE_ID,
      extractionId: EXTRACTION_ID,
      sourceLinkId: null,
      candidate: null,
    }).map((line) => line.id);
    expect(lines).not.toContain("profilePdf.summary.notCreated");
    expect(lines).toContain("profilePdf.summary.documentId");
  });

  it("treats blank fields as absent and requires the companions of a replacement", () => {
    expect(nullIfBlank("   ")).toBeNull();
    expect(nullIfBlank(` ${PROFILE_ID} `)).toBe(PROFILE_ID);
    expect(replacementReady(false, "", "")).toBe(true);
    expect(replacementReady(true, PROFILE_ID, "")).toBe(false);
    expect(replacementReady(true, PROFILE_ID, "4")).toBe(true);
  });

  it("waits for the result before the page reports it", async () => {
    serve(() => reject("conflict", "profilePdf.error.generic"));
    renderPage();
    fillDocumentId();
    fireEvent.click(screen.getByRole("button", { name: "Create profile from PDF" }));
    await waitFor(() =>
      expect(
        screen.getByText(
          "Unable to complete this profile PDF action. Check the stored document, profile ID, revision and database availability. Your entered values and previous results are preserved.",
        ),
      ).toBeInTheDocument(),
    );
  });
});
