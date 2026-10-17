import { FileText, FolderOpen, Info, RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import { formatMessage, type MessageId } from "../../i18n/messages";
import type { PageImageRefDto, StoredDocumentDto } from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { cn } from "../../lib/utils";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { Field } from "../../ui/field";
import { Label } from "../../ui/label";
import { Separator } from "../../ui/separator";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { Textarea } from "../../ui/textarea";
import { rememberDocument, StoredDocumentPicker } from "../pickers";
import * as api from "./api";
import { DocumentMeta } from "./DocumentMeta";
import { PageViewer } from "./PageViewer";
import { clampPageIndex, clampZoom, ZOOM_DEFAULT } from "./viewer";

/**
 * The operation that holds the documents workspace. Only one runs at a time (the Rust guard too). The
 * viewer's own requests take the same slot, so a page turn never overlaps an import or a preview.
 */
type Busy =
  | "choose"
  | "import"
  | "previewSelected"
  | "metadata"
  | "preview"
  | "view"
  | "page"
  | null;

/** A catalogue message with its placeholder values. */
export type Message = { id: MessageId; params?: Record<string, string | number> };

/** The PDF held by Rust for the next import or selected preview: the opaque token and the base name. */
export type HeldSelection = { selectionToken: string; displayName: string };

/** The last result: a stored document's metadata, a stored preview, or the preview of a held file. */
export type DocumentResult =
  | { kind: "metadata"; document: StoredDocumentDto }
  | { kind: "preview"; document: StoredDocumentDto; text: string; truncated: boolean }
  | { kind: "selectedPreview"; displayName: string; text: string; truncated: boolean };

/** The stored document ID is at most 36 characters. */
export const DOCUMENT_ID_MAX_LENGTH = 36;

/** The message IDs Rust sends for an action that the screen shows as they are. */
const SHOWN_AS_IS = new Set<string>([
  "documents.input.id",
  "documents.busy",
  "documents.selection.none",
  "documents.selection.invalid",
]);

/**
 * Maps a failed command to its copy. The unconfigured id is the intro of the unavailable state; the
 * input, busy and selection refusals have their own copy; every other failure has the generic copy.
 */
export function failureMessage(messageId: string): Message {
  if (messageId === "documents.intro.unavailable") {
    return { id: "documents.intro.unavailable" };
  }
  if (SHOWN_AS_IS.has(messageId)) {
    return { id: messageId as MessageId };
  }
  return { id: "documents.error.generic" };
}

/** How a status line reads: refusals and failures are errors, progress and outcomes are not. */
function toneOf(id: MessageId): "error" | "neutral" | "success" {
  if (id === "documents.error.generic" || SHOWN_AS_IS.has(id)) {
    return id === "documents.busy" ? "neutral" : "error";
  }
  return id.startsWith("documents.success.") ? "success" : "neutral";
}

/** The summary line of a stored document (copy formatted from its typed fields). */
export function summaryText(document: StoredDocumentDto): string {
  return formatMessage("documents.summary", {
    displayName: document.displayName,
    byteSize: document.byteSize,
  });
}

/** Whether the typed ID can be sent: non-blank after trimming. */
export function isIdUsable(text: string): boolean {
  return text.trim().length > 0 && text.trim().length <= DOCUMENT_ID_MAX_LENGTH;
}

/**
 * The documents screen. A PDF is chosen in a native dialog in Rust, then it
 * can be imported (its source kept) or its text previewed without an import. A stored document can be
 * read by its ID: its metadata is shown, its text can be previewed as untrusted plain text, and its
 * pages can be viewed as images. React never sees a path, a byte array or a file.
 */
export function DocumentsPage() {
  const [documentId, setDocumentId] = useState("");
  const [persist, setPersist] = useState(false);
  const [busy, setBusy] = useState<Busy>(null);
  const [selection, setSelection] = useState<HeldSelection | null>(null);
  const [result, setResult] = useState<DocumentResult | null>(null);
  const [page, setPage] = useState<PageImageRefDto | null>(null);
  const [imageFailed, setImageFailed] = useState(false);
  const [zoom, setZoom] = useState(ZOOM_DEFAULT);
  const [message, setMessage] = useState<Message | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  // Rust knows before any click whether a database is configured: the unavailable intro and the disabled
  // actions show on mount. A failed check changes nothing; the failure-driven path in `fail` still applies.
  useEffect(() => {
    let active = true;
    api.fetchDocumentsCapabilities().then(
      (capabilities) => {
        if (active && capabilities?.documents === false) {
          setUnavailable(true);
        }
      },
      () => {},
    );
    return () => {
      active = false;
    };
  }, []);

  const idUsable = isIdUsable(documentId);
  const disabled = busy !== null || unavailable;
  const storedDocument =
    result !== null && result.kind !== "selectedPreview" ? result.document : null;
  const viewerPending = busy === "view" || busy === "page" ? busy : null;

  /** Another PDF was picked: the shown file and its page no longer match the picker. */
  function onPick(next: string) {
    setDocumentId(next);
    if (next.trim() !== storedDocument?.id) {
      setResult(null);
      setPage(null);
      setImageFailed(false);
    }
  }

  /** Clears the result, the viewer and the status before an action starts. */
  function begin(kind: Exclude<Busy, null>, progress: MessageId | null) {
    setBusy(kind);
    setResult(null);
    setPage(null);
    setImageFailed(false);
    setMessage(progress === null ? null : { id: progress });
  }

  /** Shows a failed action. An unconfigured service disables the screen; the intro carries its copy. */
  function fail(error: unknown, kind: Exclude<Busy, null>) {
    const failure = isCommandError(error) ? error : unknownFailure;
    if (failure.messageId === "documents.intro.unavailable") {
      setUnavailable(true);
      setMessage(null);
      return;
    }
    if (failure.messageId === "documents.selection.none") {
      // Rust no longer holds the selection, so the screen forgets it too.
      setSelection(null);
    }
    if (kind === "choose") {
      // A refused choice drops the held selection (`document_choose_file`).
      setSelection(null);
    }
    setMessage(failureMessage(failure.messageId));
  }

  async function choose() {
    setBusy("choose");
    setMessage(null);
    try {
      const picked = await api.chooseDocument();
      if (picked === null) {
        // A cancelled dialog changes nothing.
        return;
      }
      setResult(null);
      setPage(null);
      setImageFailed(false);
      setSelection(picked);
      setMessage({ id: "documents.selection.ready" });
    } catch (error) {
      fail(error, "choose");
    } finally {
      setBusy(null);
    }
  }

  async function importSelected() {
    if (selection === null) {
      return;
    }
    const held = selection;
    begin("import", "documents.progress.import");
    try {
      const document = await api.importSelectedDocument(held.selectionToken);
      // The import spends the selection; the source file was kept.
      setSelection(null);
      setDocumentId(document.id);
      rememberDocument(document);
      setResult({ kind: "metadata", document });
      setMessage({ id: "documents.success.import" });
    } catch (error) {
      fail(error, "import");
    } finally {
      setBusy(null);
    }
  }

  async function previewSelected() {
    if (selection === null) {
      return;
    }
    const held = selection;
    begin("previewSelected", "documents.progress.preview");
    try {
      const preview = await api.previewSelectedDocument(held.selectionToken);
      setResult({
        kind: "selectedPreview",
        displayName: held.displayName,
        text: preview.text,
        truncated: preview.truncated,
      });
      setMessage({ id: "documents.success.preview" });
    } catch (error) {
      fail(error, "previewSelected");
    } finally {
      setBusy(null);
    }
  }

  async function loadMetadata() {
    const id = documentId.trim();
    begin("metadata", "documents.progress.metadata");
    try {
      const document = await api.loadDocumentMetadata(id);
      // The field takes the canonical ID the service returned.
      setDocumentId(document.id);
      rememberDocument(document);
      setResult({ kind: "metadata", document });
      setMessage({ id: "documents.success.metadata" });
    } catch (error) {
      fail(error, "metadata");
    } finally {
      setBusy(null);
    }
  }

  async function previewStored() {
    const id = documentId.trim();
    begin("preview", "documents.progress.storedPreview");
    try {
      const stored = await api.previewStoredDocument(id, persist);
      setDocumentId(stored.document.id);
      rememberDocument(stored.document);
      setResult({
        kind: "preview",
        document: stored.document,
        text: stored.extraction.text,
        truncated: stored.extraction.truncated,
      });
      setMessage({ id: "documents.success.storedPreview" });
    } catch (error) {
      fail(error, "preview");
    } finally {
      setBusy(null);
    }
  }

  /** Shows the first page of the stored document on screen. The old image stays until the new one arrives. */
  async function viewFirstPage() {
    // The picked id is enough: the page is rendered from the id, as on the Stored PDFs screen.
    const id = storedDocument?.id ?? documentId.trim();
    if (id === "") {
      return;
    }
    setBusy("view");
    setImageFailed(false);
    try {
      setPage(await api.renderDocumentPage(id, 0, zoom));
    } catch (error) {
      setPage(null);
      fail(error, "view");
    } finally {
      setBusy(null);
    }
  }

  /** Turns to another page of the shown document; the index is clamped to the page count first. */
  async function goToPage(index: number) {
    if (page === null) {
      return;
    }
    const target = clampPageIndex(index, page.pageCount);
    if (target === page.pageIndex) {
      return;
    }
    const documentRef = page.documentId;
    setBusy("page");
    setImageFailed(false);
    try {
      setPage(await api.renderDocumentPage(documentRef, target, zoom));
    } catch (error) {
      setPage(null);
      fail(error, "page");
    } finally {
      setBusy(null);
    }
  }

  async function reset() {
    setDocumentId("");
    setPersist(false);
    setSelection(null);
    setResult(null);
    setPage(null);
    setImageFailed(false);
    setZoom(ZOOM_DEFAULT);
    setMessage(null);
    try {
      await api.resetDocumentView();
    } catch (error) {
      const failure = isCommandError(error) ? error : unknownFailure;
      setMessage(failureMessage(failure.messageId));
    }
  }

  return (
    <section
      aria-labelledby="documents-heading"
      data-testid="documents-section"
      className="@container grid min-w-0 gap-4"
    >
      <h2 id="documents-heading" className="sr-only">
        {formatMessage("documents.title")}
      </h2>
      <p className="sr-only">{formatMessage("documents.a11y.group")}</p>
      {unavailable ? (
        <p
          className="flex items-start gap-2 rounded-panel border border-border bg-card px-4 py-3 text-muted-foreground"
          role="note"
        >
          <Info className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          <span>{formatMessage("documents.intro.unavailable")}</span>
        </p>
      ) : null}

      <div className="grid items-start gap-4 @[720px]:grid-cols-[minmax(0,340px)_minmax(0,1fr)]">
        <div className="min-w-0 rounded-panel border border-border bg-card">
          <div className="grid gap-3 p-4">
            <h3 className="text-base font-semibold">{formatMessage("documents.section.import")}</h3>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                id="chooseDocumentButton"
                variant="outline"
                aria-label={formatMessage("documents.a11y.choose")}
                disabled={disabled}
                onClick={() => void choose()}
              >
                <FolderOpen strokeWidth={1.75} aria-hidden />
                {formatMessage("documents.button.choose")}
              </Button>
            </div>
            <p
              id="selectedDocumentFilename"
              className={cn(
                "flex items-center gap-2 text-[13px] [overflow-wrap:anywhere]",
                selection === null && "text-muted-foreground",
              )}
              data-testid="document-selection"
              title={formatMessage("documents.selection.label")}
            >
              <FileText className="size-4 shrink-0" strokeWidth={1.75} aria-hidden />
              <span>
                {selection === null
                  ? formatMessage("documents.selection.none")
                  : selection.displayName}
              </span>
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                id="importDocumentButton"
                disabled={disabled || selection === null}
                onClick={() => void importSelected()}
              >
                {formatMessage("documents.button.import")}
              </Button>
              <Button
                type="button"
                id="previewSelectedDocumentButton"
                variant="outline"
                disabled={disabled || selection === null}
                onClick={() => void previewSelected()}
              >
                {formatMessage("documents.button.previewSelected")}
              </Button>
            </div>
          </div>

          <Separator />

          <div className="grid gap-3 p-4">
            <h3 className="text-base font-semibold">{formatMessage("documents.section.stored")}</h3>
            <Field label={formatMessage("documents.label.storedId")} id="documentIdInput">
              {(control) => (
                <StoredDocumentPicker
                  id={control.id}
                  value={documentId}
                  onChange={onPick}
                  disabled={disabled}
                />
              )}
            </Field>
            <div className="flex items-center gap-2">
              <input
                id="persistDocumentExtraction"
                type="checkbox"
                className="size-4 accent-primary"
                checked={persist}
                disabled={disabled}
                onChange={(event) => setPersist(event.target.checked)}
              />
              <Label htmlFor="persistDocumentExtraction" className="font-normal">
                {formatMessage("documents.persist.label")}
              </Label>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                id="viewDocumentPageButton"
                disabled={disabled || !idUsable}
                onClick={() => void viewFirstPage()}
              >
                {formatMessage("documents.button.view")}
              </Button>
              <Button
                type="button"
                id="loadDocumentMetadataButton"
                variant="outline"
                disabled={disabled || !idUsable}
                onClick={() => void loadMetadata()}
              >
                {formatMessage("documents.button.loadMetadata")}
              </Button>
              <Button
                type="button"
                id="previewStoredDocumentButton"
                variant="outline"
                disabled={disabled || !idUsable}
                onClick={() => void previewStored()}
              >
                {formatMessage("documents.button.previewStored")}
              </Button>
            </div>
          </div>

          <div className="grid gap-3 px-4 pb-4">
            {busy !== null && viewerPending === null ? (
              <p
                role="status"
                aria-label={formatMessage("documents.busy")}
                className="flex items-center gap-2 text-[13px]"
              >
                <Spinner />
                {message !== null ? formatMessage(message.id, message.params) : null}
              </p>
            ) : message !== null ? (
              <StatusMessage tone={toneOf(message.id)} className="text-[13px]">
                {formatMessage(message.id, message.params)}
              </StatusMessage>
            ) : null}
            <div>
              <Button
                type="button"
                id="resetDocumentButton"
                variant="ghost"
                disabled={busy !== null}
                onClick={() => void reset()}
              >
                <RotateCcw strokeWidth={1.75} aria-hidden />
                {formatMessage("documents.button.reset")}
              </Button>
            </div>
          </div>
        </div>

        <div className="grid min-w-0 gap-4">
          {result !== null ? (
            <div className="grid min-w-0 gap-3" data-testid="document-result">
              {result.kind === "selectedPreview" ? (
                <p className="text-base font-semibold [overflow-wrap:anywhere]">
                  {result.displayName}
                </p>
              ) : (
                <div className="grid gap-1" title={summaryText(result.document)}>
                  <p className="text-base font-semibold [overflow-wrap:anywhere]">
                    {result.document.displayName}
                  </p>
                  <DocumentMeta
                    pageCount={page?.documentId === result.document.id ? page.pageCount : null}
                    byteSize={result.document.byteSize}
                  />
                </div>
              )}
              {result.kind !== "metadata" ? (
                <div className="grid gap-1.5">
                  <Label htmlFor="documentPreviewText">
                    {formatMessage("documents.preview.caption")}
                  </Label>
                  <Textarea
                    id="documentPreviewText"
                    readOnly
                    aria-label={formatMessage("documents.preview.a11y")}
                    value={result.text}
                    rows={10}
                    className="font-mono text-[13px]"
                  />
                  {result.truncated ? (
                    <p className="text-[13px] text-muted-foreground">
                      {formatMessage("documents.preview.truncated")}
                    </p>
                  ) : null}
                </div>
              ) : null}
            </div>
          ) : null}

          {page !== null || viewerPending !== null || storedDocument !== null ? (
            <PageViewer
              page={page}
              pending={viewerPending}
              disabled={disabled}
              zoom={zoom}
              imageFailed={imageFailed}
              onPage={(index) => void goToPage(index)}
              onZoom={(next) => setZoom(clampZoom(next))}
              onImageError={() => setImageFailed(true)}
            />
          ) : result === null ? (
            <EmptyState
              icon={FileText}
              title={formatMessage("documents.empty.title")}
              text={formatMessage("documents.empty.text")}
            />
          ) : null}
        </div>
      </div>
    </section>
  );
}
