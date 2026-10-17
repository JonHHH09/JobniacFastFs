import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { GenerationResultDto, StoredPreviewDto } from "../../ipc/bindings";
import { GenerationPage } from "./GenerationPage";
import { buildGenerationCall, visibleFields } from "./generation";

// Synthetic identifiers and text only.
const PROFILE_ID = "00000000-0000-4000-8000-0000000000a1";
const JOB_ID = "00000000-0000-4000-8000-0000000000b1";
const RESUME_ID = "00000000-0000-4000-8000-0000000000e1";
const DOC_ID = "00000000-0000-4000-8000-0000000000c1";
const DOC_ID_DE = "00000000-0000-4000-8000-0000000000c2";

const SINGLE: GenerationResultDto = {
  variant: "single",
  resumeId: null,
  outputs: [{ documentId: DOC_ID, labelPrefix: "PDF", byteSize: 2048, pageCount: 2 }],
};

const GERMAN: GenerationResultDto = {
  variant: "germanTailored",
  resumeId: RESUME_ID,
  outputs: [
    { documentId: DOC_ID, labelPrefix: "EN", byteSize: 1000, pageCount: 1 },
    { documentId: DOC_ID_DE, labelPrefix: "DE", byteSize: 1100, pageCount: 1 },
  ],
};

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/**
 * Serves the commands of one test. The startup capability check is answered first, so the handler
 * only sees the operations the test is about.
 */
function serve(handler: (cmd: string, payload: unknown) => unknown, generation = true) {
  mockIPC((cmd, payload) =>
    cmd === "generation_capabilities_get" ? { generation } : handler(cmd, payload),
  );
}

/** Renders the page and waits until the startup check has answered. */
async function renderReady(generation = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GenerationPage />
    </QueryClientProvider>,
  );
  await waitFor(() => {
    const button = screen.getByTestId("generatePdfButton");
    if (generation) {
      expect(button).toBeEnabled();
    } else {
      expect(button).toBeDisabled();
    }
  });
}

function selectKind(value: string) {
  fireEvent.change(screen.getByTestId("generationKind"), { target: { value } });
}

function typeInto(testId: string, value: string) {
  fireEvent.change(screen.getByTestId(testId), { target: { value } });
}

describe("generation screen gating", () => {
  it("keeps every control disabled when generation is not admitted", async () => {
    serve(() => SINGLE, false);
    await renderReady(false);
    expect(screen.getByTestId("generationKind")).toBeDisabled();
    expect(screen.getByTestId("generationBody")).toBeDisabled();
  });

  it("shows the Germany fields only for the Germany kinds", async () => {
    serve(() => SINGLE);
    await renderReady();
    expect(screen.queryByTestId("generationProfileId")).toBeNull();
    expect(screen.getByTestId("generationFileName")).toBeInTheDocument();

    selectKind("masterResume");
    expect(screen.getByTestId("generationProfileId")).toBeInTheDocument();
    expect(screen.queryByTestId("generationJobId")).toBeNull();
    expect(screen.queryByTestId("generationFileName")).toBeNull();

    selectKind("germanyResume");
    expect(screen.getByTestId("generationJobId")).toBeInTheDocument();
    expect(screen.getByTestId("generationIncludeProjects")).toBeInTheDocument();
    expect(screen.queryByTestId("generationResumeId")).toBeNull();

    selectKind("germanCoverLetter");
    expect(screen.getByTestId("generationResumeId")).toBeInTheDocument();
    expect(screen.queryByTestId("generationIncludeProjects")).toBeNull();
  });
});

describe("generation requests", () => {
  it("sends only the standalone fields and shows the summary and output label", async () => {
    const invoke = vi.fn((_payload: unknown) => SINGLE);
    serve((cmd, payload) => (cmd === "generation_standalone" ? invoke(payload) : undefined));
    await renderReady();
    typeInto("generationFileName", "cv-example.pdf");
    typeInto("generationTitle", "Ada Example");
    typeInto("generationBody", "Synthetic body text.");
    fireEvent.click(screen.getByTestId("generatePdfButton"));

    await waitFor(() =>
      expect(screen.getByTestId("generationSummary").textContent).toBe(
        "PDF generated: 2 KB, 2 pages",
      ),
    );
    expect(screen.getByTestId("generationMessage").textContent).toBe("Generation completed.");
    const select = screen.getByTestId("generationOutputChoice");
    expect(within(select).getByRole("option").textContent).toBe("PDF (2 KB, 2 pages)");
    expect(invoke).toHaveBeenCalledWith({
      request: { fileName: "cv-example.pdf", title: "Ada Example", body: "Synthetic body text." },
    });
  });

  it("sends a master resume with its profile identifier only", async () => {
    const invoke = vi.fn((_payload: unknown) => SINGLE);
    serve((cmd, payload) => (cmd === "generation_resume" ? invoke(payload) : undefined));
    await renderReady();
    selectKind("masterResume");
    typeInto("generationProfileId", `  ${PROFILE_ID}  `);
    fireEvent.click(screen.getByTestId("generatePdfButton"));

    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
    expect(invoke).toHaveBeenCalledWith({
      request: { kind: "masterResume", profileId: PROFILE_ID },
    });
  });

  it("shows the generic copy for a failure and keeps the outputs empty", async () => {
    serve((cmd) => {
      if (cmd === "generation_standalone") {
        reject("internal_error", "generation.error.generic");
      }
      return undefined;
    });
    await renderReady();
    fireEvent.click(screen.getByTestId("generatePdfButton"));

    await waitFor(() =>
      expect(screen.getByTestId("generationMessage").textContent).toBe(
        "Unable to complete generation. Check the request, selected resume, database and PDF storage configuration.",
      ),
    );
    expect(screen.queryByTestId("generationOutputChoice")).toBeNull();
    expect(screen.queryByTestId("generationSummary")).toBeNull();
  });

  it("shows the generic copy for a busy refusal, never the busy accessible name", async () => {
    serve((cmd) => {
      if (cmd === "generation_standalone") {
        reject("busy", "generation.busy");
      }
      return undefined;
    });
    await renderReady();
    fireEvent.click(screen.getByTestId("generatePdfButton"));

    await waitFor(() =>
      expect(screen.getByTestId("generationMessage").textContent).toBe(
        "Unable to complete generation. Check the request, selected resume, database and PDF storage configuration.",
      ),
    );
    expect(screen.queryByText("PDF generation in progress")).toBeNull();
  });

  it("locks the controls while a generation runs and shows the progress copy", async () => {
    let finish: (value: GenerationResultDto) => void = () => undefined;
    serve((cmd) => {
      if (cmd === "generation_standalone") {
        return new Promise<GenerationResultDto>((resolve) => {
          finish = resolve;
        });
      }
      return undefined;
    });
    await renderReady();
    fireEvent.click(screen.getByTestId("generatePdfButton"));

    await waitFor(() =>
      expect(screen.getByRole("status", { name: "PDF generation in progress" })).toHaveTextContent(
        "Generating PDF…",
      ),
    );
    expect(screen.getByTestId("generatePdfButton")).toBeDisabled();
    expect(screen.getByTestId("generationKind")).toBeDisabled();
    expect(screen.getByTestId("generationBody")).toBeDisabled();

    finish(SINGLE);
    await waitFor(() => expect(screen.getByTestId("generatePdfButton")).toBeEnabled());
  });
});

describe("German results", () => {
  it("fills the empty resume field from the result and shows the bilingual summary", async () => {
    serve((cmd) => (cmd === "generation_resume" ? GERMAN : undefined));
    await renderReady();
    selectKind("germanyResume");
    typeInto("generationProfileId", PROFILE_ID);
    typeInto("generationJobId", JOB_ID);
    fireEvent.click(screen.getByTestId("generatePdfButton"));

    await waitFor(() =>
      expect(screen.getByTestId("generationSummary").textContent).toBe(
        "Bilingual Germany resume generated. Select a PDF for text preview.",
      ),
    );
    selectKind("germanCoverLetter");
    expect(screen.getByTestId("generationResumeId")).toHaveValue(RESUME_ID);
    const options = within(screen.getByTestId("generationOutputChoice")).getAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual([
      "English (1,000 B, 1 page)",
      "German (1.1 KB, 1 page)",
    ]);
  });
});

describe("generated PDF text preview", () => {
  it("previews the selected output by its document ID and shows the untrusted text", async () => {
    const preview: StoredPreviewDto = {
      document: {
        id: DOC_ID,
        displayName: "generated.pdf",
        mediaType: "application/pdf",
        byteSize: 2048,
      },
      extraction: { text: "Synthetic extracted text.", truncated: false },
    };
    const invoke = vi.fn((_payload: unknown) => preview);
    serve((cmd, payload) => {
      if (cmd === "generation_standalone") {
        return SINGLE;
      }
      if (cmd === "document_preview_stored") {
        return invoke(payload);
      }
      return undefined;
    });
    await renderReady();
    fireEvent.click(screen.getByTestId("generatePdfButton"));
    await waitFor(() => expect(screen.getByTestId("previewGeneratedPdfButton")).toBeEnabled());

    fireEvent.click(screen.getByTestId("previewGeneratedPdfButton"));
    await waitFor(() =>
      expect(screen.getByTestId("generatedPdfTextPreview")).toHaveValue(
        "Synthetic extracted text.",
      ),
    );
    expect(invoke).toHaveBeenCalledWith({
      requestJson: JSON.stringify({ documentId: DOC_ID, persist: false }),
    });
    expect(screen.getByLabelText("Untrusted text preview")).toBeInTheDocument();
  });
});

describe("generation request shape", () => {
  it("builds the standalone call from the standalone fields only", () => {
    expect(
      buildGenerationCall("standalone", {
        fileName: "a.pdf",
        title: "T",
        body: "B",
        profileId: PROFILE_ID,
        jobId: JOB_ID,
        resumeId: RESUME_ID,
        includeProjects: true,
      }),
    ).toEqual({ command: "standalone", request: { fileName: "a.pdf", title: "T", body: "B" } });
  });

  it("builds the German cover letter with its three identifiers and no project flag", () => {
    const call = buildGenerationCall("germanCoverLetter", {
      fileName: "",
      title: "",
      body: "",
      profileId: PROFILE_ID,
      jobId: JOB_ID,
      resumeId: RESUME_ID,
      includeProjects: true,
    });
    expect(call).toEqual({
      command: "resume",
      request: {
        kind: "germanCoverLetter",
        profileId: PROFILE_ID,
        jobId: JOB_ID,
        resumeId: RESUME_ID,
      },
    });
  });

  it("shows the three resume-kind fields for the German cover letter", () => {
    expect(visibleFields("germanCoverLetter")).toMatchObject({
      profileId: true,
      jobId: true,
      resumeId: true,
      includeProjects: false,
      fileName: false,
    });
  });
});

/** Serves the generation and the artifact capabilities, one standalone PDF, and the artifact commands. */
function serveArtifacts(
  caps: { artifacts: boolean; export: boolean },
  handler: (cmd: string, payload: unknown) => unknown = () => undefined,
) {
  const calls: { cmd: string; payload: unknown }[] = [];
  mockIPC((cmd, payload) => {
    calls.push({ cmd, payload });
    if (cmd === "artifact_capabilities_get") {
      return caps;
    }
    if (cmd === "generation_capabilities_get") {
      return { generation: true };
    }
    if (cmd === "generation_standalone") {
      return SINGLE;
    }
    return handler(cmd, payload);
  });
  return calls;
}

/** Generates one standalone PDF and waits for its summary. */
async function generateOne() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <GenerationPage />
    </QueryClientProvider>,
  );
  await waitFor(() => expect(screen.getByTestId("generatePdfButton")).toBeEnabled());
  fireEvent.click(screen.getByTestId("generatePdfButton"));
  await waitFor(() => expect(screen.getByTestId("generationSummary")).toBeInTheDocument());
}

describe("generated outputs and the Artifacts workspace", () => {
  it("shows no PDF page or export rows while the artifact services are not composed", async () => {
    serveArtifacts({ artifacts: false, export: false });
    await generateOne();
    await waitFor(() => expect(screen.getByTestId("generationOutputChoice")).toBeInTheDocument());
    expect(screen.queryByTestId("viewGeneratedPdfPagesButton")).toBeNull();
    expect(screen.queryByTestId("saveGeneratedPdfButton")).toBeNull();
  });

  it("views the selected output's pages by its document ID and hides Save without the export", async () => {
    const calls = serveArtifacts({ artifacts: true, export: false }, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return {
          documentId: DOC_ID,
          pageCount: 2,
          currentPage: 0,
          summary: { displayName: "generated.pdf", pageCount: 2, byteSize: "2048" },
          page: {
            documentId: DOC_ID,
            pageIndex: 0,
            pageCount: 2,
            width: 800,
            height: 600,
            revision: "1",
            url: "jobniac-page://localhost/handle/1",
          },
        };
      }
      return undefined;
    });
    await generateOne();
    await waitFor(() => expect(screen.getByTestId("viewGeneratedPdfPagesButton")).toBeEnabled());
    expect(screen.queryByTestId("saveGeneratedPdfButton")).toBeNull();

    fireEvent.click(screen.getByTestId("viewGeneratedPdfPagesButton"));
    await waitFor(() =>
      expect(screen.getByTestId("generatedArtifactSummary")).toHaveTextContent(
        /generated\.pdf.*2 pages.*2 KB/,
      ),
    );
    // The viewer is fixed to the output: it shows no ID field and sends the output's ID.
    expect(screen.queryByTestId("artifactDocumentId")).toBeNull();
    expect(calls.find((call) => call.cmd === "artifact_preview_document")?.payload).toEqual({
      requestJson: JSON.stringify({ documentId: DOC_ID }),
    });
  });

  it("saves the selected output to the file the dialog names and shows the file name", async () => {
    const calls = serveArtifacts({ artifacts: true, export: true }, (cmd) =>
      cmd === "artifact_export_document" ? { exportedName: "resume.pdf" } : undefined,
    );
    await generateOne();
    await waitFor(() => expect(screen.getByTestId("saveGeneratedPdfButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("saveGeneratedPdfButton"));

    await waitFor(() =>
      expect(screen.getByTestId("generationExportMessage")).toHaveTextContent(
        "PDF exported to resume.pdf.",
      ),
    );
    expect(calls.find((call) => call.cmd === "artifact_export_document")?.payload).toEqual({
      requestJson: JSON.stringify({ documentId: DOC_ID }),
    });
  });

  it("shows the export refusal copy when the export fails", async () => {
    serveArtifacts({ artifacts: true, export: true }, (cmd) => {
      if (cmd === "artifact_export_document") {
        reject("export_failed", "artifacts.export.failed");
      }
      return undefined;
    });
    await generateOne();
    await waitFor(() => expect(screen.getByTestId("saveGeneratedPdfButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("saveGeneratedPdfButton"));

    await waitFor(() =>
      expect(screen.getByTestId("generationExportMessage")).toHaveTextContent(
        "Unable to export PDF safely. Choose a new file in a private local folder.",
      ),
    );
  });
});
