import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ArtifactPreviewStateDto, PageImageRefDto } from "../../ipc/bindings";
import { ArtifactPage } from "./ArtifactPage";
import { ArtifactPreview } from "./ArtifactPreview";

// Synthetic identifiers and names only.
const DOC_ID = "00000000-0000-4000-8000-0000000000d1";
const CAPS_FULL = { artifacts: true, export: true };
const CAPS_VIEW_ONLY = { artifacts: true, export: false };
const CAPS_OFF = { artifacts: false, export: false };

/** A page reference as Rust returns it: the scheme URL and the string revision, never a path. */
function pageRef(pageIndex: number, pageCount: number, revision: number): PageImageRefDto {
  return {
    documentId: DOC_ID,
    pageIndex,
    pageCount,
    width: 800,
    height: 600,
    revision: String(revision),
    url: `jobniac-page://localhost/artifact/handle-${revision}/${revision}`,
  };
}

const PREVIEW: ArtifactPreviewStateDto = {
  documentId: DOC_ID,
  pageCount: 2,
  currentPage: 0,
  summary: { displayName: "Example CV.pdf", pageCount: 2, byteSize: "2048" },
  page: pageRef(0, 2, 1),
};

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/** One IPC call of a test, recorded so the tests can check what crossed the boundary. */
type Call = { cmd: string; payload: unknown };

type Caps = { artifacts: boolean; export: boolean };

/**
 * Serves the commands of one test. The startup capability check is answered from `caps`; the handler
 * sees the rest. Every call is recorded.
 */
function serve(
  caps: Caps,
  handler: (cmd: string, payload: unknown) => unknown = () => undefined,
): Call[] {
  const calls: Call[] = [];
  mockIPC((cmd, payload) => {
    calls.push({ cmd, payload });
    return cmd === "artifact_capabilities_get" ? caps : handler(cmd, payload);
  });
  return calls;
}

/** Renders the screen and waits until the startup check has answered and React has shown it. */
async function renderReady(calls: Call[]) {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <ArtifactPage />
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(calls.some((call) => call.cmd === "artifact_capabilities_get")).toBe(true),
  );
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function typeId(value: string) {
  fireEvent.change(screen.getByTestId("artifactDocumentId"), { target: { value } });
}

/** Loads the sample document through the View button. */
async function viewSample() {
  fireEvent.click(screen.getByTestId("previewArtifactButton"));
  await waitFor(() => expect(screen.getByTestId("artifactSummary")).toBeInTheDocument());
}

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("artifact screen gating", () => {
  it("keeps the ID field and the buttons disabled when the service is not composed", async () => {
    const calls = serve(CAPS_OFF);
    await renderReady(calls);
    expect(screen.getByTestId("artifactDocumentId")).toBeDisabled();
    expect(screen.getByTestId("previewArtifactButton")).toBeDisabled();
    expect(screen.queryByTestId("saveArtifactButton")).toBeNull();
  });

  it("shows the viewer without the Save control when only the viewer is admitted", async () => {
    const calls = serve(CAPS_VIEW_ONLY);
    await renderReady(calls);
    typeId(DOC_ID);
    expect(screen.getByTestId("previewArtifactButton")).toBeEnabled();
    expect(screen.queryByTestId("saveArtifactButton")).toBeNull();
  });

  it("shows Save when the export is admitted, and needs a non-blank ID for both buttons", async () => {
    const calls = serve(CAPS_FULL);
    await renderReady(calls);
    expect(screen.getByTestId("saveArtifactButton")).toBeDisabled();
    typeId("   ");
    expect(screen.getByTestId("previewArtifactButton")).toBeDisabled();
    expect(screen.getByTestId("saveArtifactButton")).toBeDisabled();
    typeId(DOC_ID);
    expect(screen.getByTestId("previewArtifactButton")).toBeEnabled();
    expect(screen.getByTestId("saveArtifactButton")).toBeEnabled();
  });

  it("sends only a canonical document ID: any other text leaves both buttons disabled", async () => {
    const calls = serve(CAPS_FULL);
    await renderReady(calls);
    typeId(`${DOC_ID}-and-more-text-past-the-id-length`);
    expect(screen.getByTestId("previewArtifactButton")).toBeDisabled();
    expect(screen.getByTestId("saveArtifactButton")).toBeDisabled();
  });
});

describe("preview and paging", () => {
  it("sends the trimmed ID by reference, then shows the summary, the page and the ready copy", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) =>
      cmd === "artifact_preview_document" ? PREVIEW : undefined,
    );
    await renderReady(calls);
    typeId(`  ${DOC_ID}  `);
    await viewSample();

    const summary = screen.getByTestId("artifactSummary");
    expect(summary).toHaveTextContent("Example CV.pdf");
    expect(summary).toHaveTextContent("2 pages");
    expect(summary).toHaveTextContent("2 KB");
    expect(screen.getByTestId("artifact-pager-label")).toHaveTextContent("1 / 2");
    expect(screen.getByTestId("artifact-viewer-image")).toHaveAttribute(
      "src",
      "jobniac-page://localhost/artifact/handle-1/1",
    );
    expect(screen.getByTestId("artifactMessage")).toHaveTextContent("PDF preview ready.");
  });

  it("sends the document ID as a JSON request and never a path", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) =>
      cmd === "artifact_preview_document" ? PREVIEW : undefined,
    );
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    const preview = calls.find((call) => call.cmd === "artifact_preview_document");
    expect(preview?.payload).toEqual({ requestJson: JSON.stringify({ documentId: DOC_ID }) });
  });

  it("turns the page within the page count and disables the pager at both ends", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd, payload) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_show_page") {
        const index = JSON.parse((payload as { requestJson: string }).requestJson).pageIndex;
        return pageRef(index, 2, index + 1);
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    expect(screen.getByRole("button", { name: "Previous" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(screen.getByTestId("artifact-pager-label")).toHaveTextContent("2 / 2"),
    );
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Previous" })).toBeEnabled();
    const turns = calls.filter((call) => call.cmd === "artifact_show_page");
    expect(turns[turns.length - 1]?.payload).toEqual({
      requestJson: JSON.stringify({ documentId: DOC_ID, pageIndex: 1 }),
    });
  });

  it("clears the viewer when a page turn fails, so the pager is hidden", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_show_page") {
        reject("preview_failed", "artifacts.preview.failed");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent(
        "Unable to preview this PDF safely.",
      ),
    );
    expect(screen.queryByTestId("artifact-pager-label")).toBeNull();
    expect(screen.queryByTestId("artifactSummary")).toBeNull();
  });

  it("shows the failed-preview copy and no viewer for an unknown document", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        reject("not_found", "artifacts.preview.failed");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("previewArtifactButton"));

    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent(
        "Unable to preview this PDF safely.",
      ),
    );
    expect(screen.queryByTestId("artifact-viewer-image")).toBeNull();
  });

  it("shows the input copy when Rust refuses the ID", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        reject("validation_error", "artifacts.input.id");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("previewArtifactButton"));

    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent(
        "Choose a valid stored document ID.",
      ),
    );
  });

  it("shows nothing for a busy refusal and keeps the controls locked while a preview runs", async () => {
    const pending = deferred<ArtifactPreviewStateDto>();
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return pending.promise;
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("previewArtifactButton"));

    await waitFor(() =>
      expect(screen.getByRole("status", { name: "PDF operation in progress" })).toHaveTextContent(
        "Loading PDF page…",
      ),
    );
    expect(screen.getByTestId("previewArtifactButton")).toBeDisabled();
    expect(screen.getByTestId("saveArtifactButton")).toBeDisabled();
    expect(screen.getByTestId("artifactDocumentId")).toBeDisabled();

    pending.resolve(PREVIEW);
    await waitFor(() => expect(screen.getByTestId("artifactSummary")).toBeInTheDocument());
    expect(screen.getByTestId("previewArtifactButton")).toBeEnabled();
  });

  it("shows the image failure copy when the browser cannot show the page", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) =>
      cmd === "artifact_preview_document" ? PREVIEW : undefined,
    );
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.error(screen.getByTestId("artifact-viewer-image"));
    expect(screen.getByTestId("artifact-viewer-error")).toHaveTextContent(
      "This page image could not be shown.",
    );
  });
});

describe("export", () => {
  it("publishes the document and shows the exported file name, keeping the viewer", async () => {
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_export_document") {
        return { exportedName: "copy.pdf" };
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.click(screen.getByTestId("saveArtifactButton"));
    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent("PDF exported to copy.pdf."),
    );
    expect(screen.getByTestId("artifactSummary")).toBeInTheDocument();
    expect(calls.find((call) => call.cmd === "artifact_export_document")?.payload).toEqual({
      requestJson: JSON.stringify({ documentId: DOC_ID }),
    });
  });

  it("shows nothing for a cancelled dialog and changes nothing", async () => {
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_export_document") {
        return null;
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.click(screen.getByTestId("saveArtifactButton"));
    await waitFor(() =>
      expect(calls.some((call) => call.cmd === "artifact_export_document")).toBe(true),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(screen.queryByTestId("artifactMessage")).toBeNull();
    expect(screen.getByTestId("artifactSummary")).toBeInTheDocument();
  });

  it("shows the refusal copy for an export that was not published", async () => {
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_export_document") {
        reject("export_failed", "artifacts.export.failed");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("saveArtifactButton"));

    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent(
        "Unable to export PDF safely. Choose a new file in a private local folder.",
      ),
    );
  });

  it("shows the uncertainty copy when the completion could not be verified", async () => {
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_export_document") {
        reject("export_uncertain", "artifacts.export.uncertain");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("saveArtifactButton"));

    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent(
        "Export may have been saved, but completion could not be verified.",
      ),
    );
  });

  it("shows the export progress copy while the dialog and the export run", async () => {
    const pending = deferred<{ exportedName: string } | null>();
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_export_document") {
        return pending.promise;
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("saveArtifactButton"));

    await waitFor(() =>
      expect(screen.getByRole("status", { name: "PDF operation in progress" })).toHaveTextContent(
        "Exporting PDF…",
      ),
    );
    pending.resolve(null);
    await waitFor(() => expect(screen.getByTestId("artifactStatus")).toBeEmptyDOMElement());
  });
});

describe("shared session and live region", () => {
  it("keeps the page and summary when a preview is refused as busy", async () => {
    let previews = 0;
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        previews += 1;
        if (previews === 2) {
          reject("busy", "artifacts.busy");
        }
        return PREVIEW;
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.click(screen.getByTestId("previewArtifactButton"));
    await waitFor(() => expect(previews).toBe(2));
    await waitFor(() => expect(screen.getByTestId("previewArtifactButton")).toBeEnabled());
    expect(screen.getByTestId("artifactSummary")).toBeInTheDocument();
    expect(screen.getByTestId("artifact-pager-label")).toHaveTextContent("1 / 2");
  });

  it("shows no stale outcome when a preview is refused as busy after a failed export", async () => {
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_export_document") {
        reject("failed", "artifacts.export.failed");
      }
      if (cmd === "artifact_preview_document") {
        reject("busy", "artifacts.busy");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    fireEvent.click(screen.getByTestId("saveArtifactButton"));
    await waitFor(() => expect(screen.getByTestId("artifactMessage")).toBeInTheDocument());
    fireEvent.click(screen.getByTestId("previewArtifactButton"));
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === "artifact_preview_document")).toBe(true),
    );
    await waitFor(() => expect(screen.getByTestId("artifactStatus")).toBeEmptyDOMElement());
    expect(screen.queryByTestId("artifactMessage")).toBeNull();
  });

  it("lets a fixed viewer reload after a failed load", async () => {
    let previews = 0;
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        previews += 1;
        if (previews === 1) {
          reject("failed", "artifacts.preview.failed");
        }
        return PREVIEW;
      }
      return undefined;
    });
    render(<ArtifactPreview documentId={DOC_ID} available save={false} />);
    await waitFor(() => expect(screen.getByTestId("retryArtifactButton")).toBeEnabled());
    fireEvent.click(screen.getByTestId("retryArtifactButton"));
    await waitFor(() => expect(screen.getByTestId("artifactSummary")).toBeInTheDocument());
    expect(screen.queryByTestId("retryArtifactButton")).toBeNull();
    expect(calls.filter((c) => c.cmd === "artifact_preview_document")).toHaveLength(2);
  });

  it("keeps the page when a page turn is refused as busy", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_show_page") {
        reject("busy", "artifacts.busy");
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => expect(calls.some((call) => call.cmd === "artifact_show_page")).toBe(true));
    await waitFor(() => expect(screen.getByRole("button", { name: "Next" })).toBeEnabled());
    expect(screen.getByTestId("artifactSummary")).toBeInTheDocument();
    expect(screen.getByTestId("artifact-pager-label")).toHaveTextContent("1 / 2");
  });

  it("clears the viewer when a page turn returns another document's page", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_show_page") {
        return { ...pageRef(1, 2, 2), documentId: "00000000-0000-4000-8000-0000000000d2" };
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent(
        "Unable to preview this PDF safely.",
      ),
    );
    expect(screen.queryByTestId("artifact-pager-label")).toBeNull();
    expect(screen.queryByTestId("artifactSummary")).toBeNull();
  });

  it("mounts the live region before the outcome text arrives", async () => {
    const calls = serve(CAPS_VIEW_ONLY, (cmd) =>
      cmd === "artifact_preview_document" ? PREVIEW : undefined,
    );
    await renderReady(calls);
    const region = screen.getByTestId("artifactStatus");
    expect(region).toBeEmptyDOMElement();
    typeId(DOC_ID);
    await viewSample();
    expect(screen.getByTestId("artifactStatus")).toBe(region);
    expect(region).toHaveTextContent("PDF preview ready.");
  });
});

describe("the IPC boundary", () => {
  it("never sends a path-like value through invoke", async () => {
    const calls = serve(CAPS_FULL, (cmd) => {
      if (cmd === "artifact_preview_document") {
        return PREVIEW;
      }
      if (cmd === "artifact_export_document") {
        return { exportedName: "copy.pdf" };
      }
      if (cmd === "artifact_show_page") {
        return pageRef(1, 2, 2);
      }
      return undefined;
    });
    await renderReady(calls);
    typeId(DOC_ID);
    await viewSample();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(screen.getByTestId("artifact-pager-label")).toHaveTextContent("2 / 2"),
    );
    fireEvent.click(screen.getByTestId("saveArtifactButton"));
    await waitFor(() =>
      expect(screen.getByTestId("artifactMessage")).toHaveTextContent("PDF exported to copy.pdf."),
    );

    const artifactCalls = calls.filter(
      (call) => call.cmd.startsWith("artifact_") && call.cmd !== "artifact_capabilities_get",
    );
    // Preview, page turn and export: three requests, each checked below.
    expect(artifactCalls.map((call) => call.cmd)).toEqual([
      "artifact_preview_document",
      "artifact_show_page",
      "artifact_export_document",
    ]);
    for (const call of artifactCalls) {
      // Each request is one JSON text of identifiers and indexes; it names no file.
      const request = (call.payload as { requestJson: string }).requestJson;
      expect(request).not.toMatch(/[\\/]|\.pdf|[A-Za-z]:[\\/]/i);
      expect(Object.keys(JSON.parse(request))).not.toContain("path");
    }
  });
});
