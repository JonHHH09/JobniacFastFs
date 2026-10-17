import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type {
  PageImageRefDto,
  StoredDocumentDto,
  StoredPreviewDto,
  TextPreviewDto,
} from "../../ipc/bindings";
import { pickerKeys } from "../pickers/ProfilePicker";
import { DocumentsPage, failureMessage, isIdUsable, summaryText } from "./DocumentsPage";

// Synthetic identifiers, tokens and text only.
const DOCUMENT_ID = "00000000-0000-4000-8000-0000000000d1";
const SELECTION_TOKEN = "00000000-0000-4000-8000-0000000000e1";
const PAGE_URL = "http://jobniac-page.localhost/handle-1/7";

const METADATA: StoredDocumentDto = {
  id: DOCUMENT_ID,
  displayName: "synthetic-cv.pdf",
  mediaType: "application/pdf",
  byteSize: 2048,
};

/** The requests the page sent, in order, with their decoded payloads. */
let sent: Array<{ cmd: string; body: unknown }> = [];

/** The answer of the mount-time capabilities check; a test sets it to `false` for an unconfigured database. */
let documentsAvailable = true;

function serve(handler: (cmd: string, body: unknown) => unknown) {
  sent = [];
  mockIPC((cmd, payload) => {
    if (cmd === "documents_capabilities_get") {
      return { documents: documentsAvailable };
    }
    // The stored-PDF picker lists while it is open; the listing is not an action of the page.
    if (cmd === "documents_list") {
      return { documents: [], nextCursor: null };
    }
    const body =
      payload && typeof payload === "object" && "requestJson" in payload
        ? JSON.parse((payload as { requestJson: string }).requestJson)
        : payload;
    sent.push({ cmd, body });
    return handler(cmd, body);
  });
}

function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/** The last request the page sent. */
function lastSent() {
  return sent[sent.length - 1];
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The name the picker shows for the document is already known, so it asks Rust for nothing.
  client.setQueryData(pickerKeys.document(DOCUMENT_ID), METADATA);
  return render(
    <QueryClientProvider client={client}>
      <DocumentsPage />
    </QueryClientProvider>,
  );
}

function pageRef(pageIndex: number, pageCount = 3): PageImageRefDto {
  return {
    documentId: DOCUMENT_ID,
    pageIndex,
    pageCount,
    width: 600,
    height: 800,
    revision: "7",
    url: PAGE_URL,
  };
}

/** A promise the test resolves by hand, to hold an action in its busy state. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** Loads the metadata of the stored document so that the viewer and the stored actions are reachable. */
async function loadStoredDocument() {
  fireEvent.change(screen.getByLabelText("Stored PDF"), {
    target: { value: DOCUMENT_ID },
  });
  fireEvent.click(screen.getByRole("button", { name: "Load metadata" }));
  await screen.findByTestId("document-result");
}

/** Chooses a PDF through the (mocked) native dialog; the screen only receives the token and name. */
async function chooseSynthetic() {
  fireEvent.click(screen.getByRole("button", { name: "Choose a PDF without importing it" }));
  await screen.findByText("File selected. Choose Import PDF or Preview selected PDF to continue.");
}

describe("documents page", () => {
  it("sends the trimmed ID, shows the summary and fills the field with the canonical ID", async () => {
    serve((cmd) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    const input = screen.getByLabelText("Stored PDF");
    fireEvent.change(input, { target: { value: `  ${DOCUMENT_ID.toUpperCase()}  ` } });
    fireEvent.click(screen.getByRole("button", { name: "Load metadata" }));

    await waitFor(() => expect(screen.getByTestId("document-result")).toBeInTheDocument());
    expect(sent[0]).toEqual({
      cmd: "document_load_metadata",
      body: { documentId: DOCUMENT_ID.toUpperCase() },
    });
    expect(
      within(screen.getByTestId("document-result")).getByText("synthetic-cv.pdf"),
    ).toBeInTheDocument();
    expect(within(screen.getByTestId("document-result")).getByText("2 KB")).toBeInTheDocument();
    // The picker shows the name of the stored PDF whose ID the field holds.
    await waitFor(() =>
      expect(screen.getByLabelText("Stored PDF")).toHaveValue("synthetic-cv.pdf"),
    );
  });

  it("previews the stored text as untrusted plain text and sends the persist flag", async () => {
    const preview: StoredPreviewDto = {
      document: METADATA,
      extraction: { text: "Synthetic text <b>kept as text</b>", truncated: true },
    };
    serve((cmd) => {
      if (cmd === "document_preview_stored") {
        return preview;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    fireEvent.change(screen.getByLabelText("Stored PDF"), {
      target: { value: DOCUMENT_ID },
    });
    fireEvent.click(screen.getByLabelText("Persist extracted text in the database"));
    fireEvent.click(screen.getByRole("button", { name: "Preview stored PDF" }));

    await waitFor(() => expect(screen.getByTestId("document-result")).toBeInTheDocument());
    expect(sent[0]?.body).toEqual({ documentId: DOCUMENT_ID, persist: true });
    const text = screen.getByLabelText("Untrusted PDF plain text preview");
    expect(text).toHaveValue("Synthetic text <b>kept as text</b>");
    expect(screen.getByText("Preview truncated at the character limit.")).toBeInTheDocument();
    expect(screen.getByText("Untrusted stored PDF text preview.")).toBeInTheDocument();
  });

  it("sends no request for text that is not a document ID and keeps the typed value", async () => {
    serve(() => reject("validation_error", "documents.input.id"));
    renderPage();
    fireEvent.change(screen.getByLabelText("Stored PDF"), {
      target: { value: "not-an-id" },
    });

    expect(screen.getByRole("button", { name: "Load metadata" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Preview stored PDF" })).toBeDisabled();
    expect(screen.getByLabelText("Stored PDF")).toHaveValue("not-an-id");
    expect(sent).toHaveLength(0);
  });

  it("shows the input copy when Rust refuses a well-formed ID", async () => {
    serve(() => reject("validation_error", "documents.input.id"));
    renderPage();
    fireEvent.change(screen.getByLabelText("Stored PDF"), {
      target: { value: DOCUMENT_ID },
    });
    fireEvent.click(screen.getByRole("button", { name: "Load metadata" }));

    expect(await screen.findByText("Enter a valid stored document ID.")).toBeInTheDocument();
  });

  it("shows the unavailable intro and disables the actions on mount when Rust reports no database", async () => {
    documentsAvailable = false;
    try {
      serve(() => undefined);
      renderPage();

      expect(
        await screen.findByText(
          "Document service is unavailable. Configure the local database and an explicit import root for file actions.",
        ),
      ).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Load metadata" })).toBeDisabled();
      expect(screen.getByLabelText("Stored PDF")).toBeDisabled();
      expect(sent.map((request) => request.cmd)).toEqual([]);
    } finally {
      documentsAvailable = true;
    }
  });

  it("shows the unavailable intro and disables the actions when no database is configured", async () => {
    serve(() => reject("not_configured", "documents.intro.unavailable"));
    renderPage();
    fireEvent.change(screen.getByLabelText("Stored PDF"), {
      target: { value: DOCUMENT_ID },
    });
    fireEvent.click(screen.getByRole("button", { name: "Load metadata" }));

    expect(
      await screen.findByText(
        "Document service is unavailable. Configure the local database and an explicit import root for file actions.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Load metadata" })).toBeDisabled();
    expect(screen.getByLabelText("Stored PDF")).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Choose a PDF without importing it" }),
    ).toBeDisabled();
  });

  it("keeps the generic copy for a core failure and clears the previous result first", async () => {
    let calls = 0;
    serve(() => {
      calls += 1;
      if (calls === 1) {
        return METADATA;
      }
      return reject("not_found", "documents.error.generic");
    });
    renderPage();
    fireEvent.change(screen.getByLabelText("Stored PDF"), {
      target: { value: DOCUMENT_ID },
    });
    fireEvent.click(screen.getByRole("button", { name: "Load metadata" }));
    await screen.findByTestId("document-result");
    fireEvent.click(screen.getByRole("button", { name: "Load metadata" }));

    expect(
      await screen.findByText(
        "Unable to complete this document action. Check the PDF, configured import root, stored document ID and database availability.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("document-result")).not.toBeInTheDocument();
  });

  it("starts with no selection and both selection actions disabled", () => {
    serve(() => {
      throw new Error("no command expected");
    });
    renderPage();
    expect(screen.getByText("No PDF selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Preview selected PDF" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "View PDF" })).toBeDisabled();
    expect(sent).toHaveLength(0);
  });

  it("changes nothing when the native dialog is cancelled", async () => {
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return null;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Choose a PDF without importing it" }));

    await waitFor(() => expect(sent).toHaveLength(1));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Choose a PDF without importing it" }),
      ).toBeEnabled(),
    );
    expect(screen.getByText("No PDF selected")).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeDisabled();
  });

  it("shows the file name and enables import and preview after a choice", async () => {
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();

    expect(screen.getByText("synthetic-cv.pdf")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Preview selected PDF" })).toBeEnabled();
    // The choose command takes no arguments: the path is chosen and kept in Rust.
    expect(sent.map((entry) => entry.cmd)).toEqual(["document_choose_file"]);
  });

  it("imports the held PDF with its token only, then spends the selection and fills the ID", async () => {
    serve((cmd, body) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_import_selected") {
        expect(body).toEqual({ selectionToken: SELECTION_TOKEN });
        return METADATA;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Import PDF" }));

    expect(
      await screen.findByText("PDF imported. The source file was preserved."),
    ).toBeInTheDocument();
    expect(
      within(screen.getByTestId("document-result")).getByText("synthetic-cv.pdf"),
    ).toBeInTheDocument();
    expect(within(screen.getByTestId("document-result")).getByText("2 KB")).toBeInTheDocument();
    // The picker shows the name of the stored PDF whose ID the field holds.
    await waitFor(() =>
      expect(screen.getByLabelText("Stored PDF")).toHaveValue("synthetic-cv.pdf"),
    );
    expect(screen.getByText("No PDF selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "View PDF" })).toBeEnabled();
  });

  it("previews the held PDF without importing it and keeps the selection", async () => {
    const preview: TextPreviewDto = { text: "Synthetic resume text", truncated: false };
    serve((cmd, body) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_preview_selected") {
        expect(body).toEqual({ selectionToken: SELECTION_TOKEN });
        return preview;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Preview selected PDF" }));

    expect(
      await screen.findByText("Untrusted text preview. This action did not import the source PDF."),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Untrusted PDF plain text preview")).toHaveValue(
      "Synthetic resume text",
    );
    expect(screen.queryByText("Preview truncated at the character limit.")).not.toBeInTheDocument();
    expect(screen.getByTestId("document-selection")).toHaveTextContent("synthetic-cv.pdf");
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeEnabled();
    expect(sent.map((entry) => entry.cmd)).toEqual([
      "document_choose_file",
      "document_preview_selected",
    ]);
  });

  it("keeps the selection and shows the generic copy when an import fails", async () => {
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_import_selected") {
        return reject("validation_error", "documents.error.generic");
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Import PDF" }));

    expect(
      await screen.findByText(
        "Unable to complete this document action. Check the PDF, configured import root, stored document ID and database availability.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("synthetic-cv.pdf")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeEnabled();
  });

  it("forgets the selection when Rust reports that none is held", async () => {
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_preview_selected") {
        return reject("validation_error", "documents.selection.none");
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Preview selected PDF" }));

    await waitFor(() =>
      expect(screen.getByTestId("document-selection")).toHaveTextContent("No PDF selected"),
    );
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeDisabled();
  });

  it("drops the selection when a choice is refused", async () => {
    let choices = 0;
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        choices += 1;
        if (choices === 1) {
          return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
        }
        return reject("validation_error", "documents.selection.invalid");
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Choose a PDF without importing it" }));

    expect(await screen.findByText("Choose a local PDF file.")).toBeInTheDocument();
    expect(screen.getByText("No PDF selected")).toBeInTheDocument();
  });

  it("gates every action while an import runs and shows its progress copy", async () => {
    const held = deferred<typeof METADATA>();
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_import_selected") {
        return held.promise;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Import PDF" }));

    expect(await screen.findByText("Importing the selected PDF…")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Choose a PDF without importing it" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "Preview selected PDF" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Load metadata" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reset document view" })).toBeDisabled();
    expect(screen.getByLabelText("Stored PDF")).toBeDisabled();

    held.resolve(METADATA);
    expect(
      await screen.findByText("PDF imported. The source file was preserved."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reset document view" })).toBeEnabled();
  });

  it("shows the preview progress copy while a selected preview runs", async () => {
    const held = deferred<TextPreviewDto>();
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_preview_selected") {
        return held.promise;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Preview selected PDF" }));

    expect(await screen.findByText("Reading selected PDF text…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import PDF" })).toBeDisabled();
    held.resolve({ text: "Synthetic", truncated: false });
    expect(await screen.findByLabelText("Untrusted PDF plain text preview")).toHaveValue(
      "Synthetic",
    );
  });

  it("resets the whole view and sends no arguments to the reset command", async () => {
    serve((cmd) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        return pageRef(0);
      }
      if (cmd === "document_reset") {
        return null;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    await loadStoredDocument();
    fireEvent.click(screen.getByLabelText("Persist extracted text in the database"));
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    await screen.findByRole("img", { name: "PDF page image" });

    fireEvent.click(screen.getByRole("button", { name: "Reset document view" }));

    await waitFor(() => expect(screen.getByText("No PDF selected")).toBeInTheDocument());
    expect(screen.getByLabelText("Stored PDF")).toHaveValue("");
    expect(screen.getByLabelText("Persist extracted text in the database")).not.toBeChecked();
    expect(screen.queryByTestId("document-result")).not.toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "PDF page image" })).not.toBeInTheDocument();
    expect(lastSent()?.cmd).toBe("document_reset");
  });

  it("shows the first page of a stored document and pages within its bounds", async () => {
    serve((cmd, body) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        const { pageIndex } = body as { pageIndex: number };
        return pageRef(pageIndex);
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await loadStoredDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));

    const image = await screen.findByRole("img", { name: "PDF page image" });
    expect(image).toHaveAttribute("src", PAGE_URL);
    expect(image).toHaveAttribute("width", "600");
    expect(image.style.aspectRatio).toBe("600 / 800");
    expect(screen.getByTestId("document-pager-label")).toHaveTextContent("1 / 3");
    expect(lastSent()).toEqual({
      cmd: "document_render_page",
      body: { documentId: DOCUMENT_ID, pageIndex: 0, zoom: 1 },
    });
    expect(screen.getByRole("button", { name: "Previous" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(screen.getByTestId("document-pager-label")).toHaveTextContent("2 / 3"),
    );
    expect(lastSent()).toEqual({
      cmd: "document_render_page",
      body: { documentId: DOCUMENT_ID, pageIndex: 1, zoom: 1 },
    });

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(screen.getByTestId("document-pager-label")).toHaveTextContent("3 / 3"),
    );
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Previous" })).toBeEnabled();
  });

  it("keeps the old page on screen while the next page loads", async () => {
    const next = deferred<PageImageRefDto>();
    serve((cmd, body) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        const { pageIndex } = body as { pageIndex: number };
        return pageIndex === 0 ? pageRef(0) : next.promise;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await loadStoredDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    await screen.findByRole("img", { name: "PDF page image" });

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Loading PDF page…")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "PDF page image" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "75%" })).toBeDisabled();

    next.resolve(pageRef(1));
    await waitFor(() =>
      expect(screen.getByTestId("document-pager-label")).toHaveTextContent("2 / 3"),
    );
  });

  it("clamps zoom to the offered range and never asks Rust for a new render", async () => {
    serve((cmd) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        return pageRef(0);
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await loadStoredDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    await screen.findByRole("img", { name: "PDF page image" });

    // Only the offered levels exist, from 25% to 100%, and 100% is in effect.
    expect(screen.getAllByRole("radio").map((radio) => radio.textContent)).toEqual([
      "25%",
      "50%",
      "75%",
      "100%",
    ]);
    expect(screen.getByRole("radio", { name: "100%" })).toBeChecked();
    const renders = sent.filter((entry) => entry.cmd === "document_render_page").length;

    fireEvent.click(screen.getByRole("radio", { name: "25%" }));
    expect(screen.getByRole("radio", { name: "25%" })).toBeChecked();
    expect(screen.getByTestId("document-viewer-image").style.width).toBe("25%");
    expect(sent.filter((entry) => entry.cmd === "document_render_page")).toHaveLength(renders);
  });

  it("sends the zoom in effect with the next page request", async () => {
    serve((cmd, body) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        return pageRef((body as { pageIndex: number }).pageIndex);
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await loadStoredDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    await screen.findByRole("img", { name: "PDF page image" });
    fireEvent.click(screen.getByRole("radio", { name: "75%" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));

    await waitFor(() =>
      expect(lastSent()).toEqual({
        cmd: "document_render_page",
        body: { documentId: DOCUMENT_ID, pageIndex: 1, zoom: 0.75 },
      }),
    );
  });

  it("shows the image failure copy and keeps the pager usable", async () => {
    serve((cmd, body) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        return pageRef((body as { pageIndex: number }).pageIndex);
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await loadStoredDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    const image = await screen.findByRole("img", { name: "PDF page image" });

    fireEvent.error(image);
    expect(await screen.findByText("This page image could not be shown.")).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "PDF page image" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next" })).toBeEnabled();
  });

  it("clears the viewer and shows the generic copy when a render fails", async () => {
    let renders = 0;
    serve((cmd) => {
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      if (cmd === "document_render_page") {
        renders += 1;
        if (renders === 1) {
          return pageRef(0);
        }
        return reject("preview_failed", "documents.error.generic");
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await loadStoredDocument();
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    await screen.findByRole("img", { name: "PDF page image" });

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(
      await screen.findByText(
        "Unable to complete this document action. Check the PDF, configured import root, stored document ID and database availability.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "PDF page image" })).not.toBeInTheDocument();
    expect(screen.getByTestId("document-viewer-empty")).toHaveTextContent("No page loaded.");
  });

  it("never sends a path-like value through invoke", async () => {
    serve((cmd, body) => {
      if (cmd === "document_choose_file") {
        return { selectionToken: SELECTION_TOKEN, displayName: "synthetic-cv.pdf" };
      }
      if (cmd === "document_import_selected") {
        return METADATA;
      }
      if (cmd === "document_preview_selected") {
        return { text: "Synthetic", truncated: false };
      }
      if (cmd === "document_render_page") {
        return pageRef((body as { pageIndex: number }).pageIndex);
      }
      if (cmd === "document_load_metadata") {
        return METADATA;
      }
      throw new Error(`unexpected ${cmd}`);
    });
    renderPage();
    await chooseSynthetic();
    fireEvent.click(screen.getByRole("button", { name: "Preview selected PDF" }));
    await screen.findByLabelText("Untrusted PDF plain text preview");
    fireEvent.click(screen.getByRole("button", { name: "Import PDF" }));
    await screen.findByText("PDF imported. The source file was preserved.");
    fireEvent.click(screen.getByRole("button", { name: "View PDF" }));
    await screen.findByRole("img", { name: "PDF page image" });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByTestId("document-pager-label");

    expect(sent.length).toBeGreaterThan(4);
    const pathLike = /[\\/]|:\\|\.pdf|^[A-Za-z]:|file:|\bhttps?:/i;
    for (const entry of sent) {
      expect(JSON.stringify(entry.body ?? null)).not.toMatch(pathLike);
    }
    // The file name is the only text React receives about the PDF; it never goes back to Rust.
    expect(sent.some((entry) => JSON.stringify(entry.body ?? null).includes("synthetic-cv"))).toBe(
      false,
    );
  });
});

describe("documents helpers", () => {
  it("accepts only a non-blank ID of at most 36 characters", () => {
    expect(isIdUsable("   ")).toBe(false);
    expect(isIdUsable(DOCUMENT_ID)).toBe(true);
    expect(isIdUsable("x".repeat(37))).toBe(false);
  });

  it("maps the refusal, busy and selection ids to their own copy and everything else to the generic copy", () => {
    expect(failureMessage("documents.busy")).toEqual({ id: "documents.busy" });
    expect(failureMessage("documents.input.id")).toEqual({ id: "documents.input.id" });
    expect(failureMessage("documents.selection.none")).toEqual({ id: "documents.selection.none" });
    expect(failureMessage("documents.selection.invalid")).toEqual({
      id: "documents.selection.invalid",
    });
    expect(failureMessage("something.else")).toEqual({ id: "documents.error.generic" });
  });

  it("formats the summary from the typed fields", () => {
    expect(summaryText(METADATA)).toBe("synthetic-cv.pdf · application/pdf · 2048 bytes");
  });
});
