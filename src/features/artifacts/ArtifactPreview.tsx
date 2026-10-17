import { CircleAlert, CircleCheck, Download, FileText } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { formatMessage } from "../../i18n/messages";
import type { ArtifactSummaryDto, PageImageRefDto } from "../../ipc/bindings";
import { cn } from "../../lib/utils";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { Field } from "../../ui/field";
import { Spinner } from "../../ui/spinner";
import { DocumentMeta } from "../documents/DocumentMeta";
import { PageViewer } from "../documents/PageViewer";
import { clampPageIndex, clampZoom, ZOOM_DEFAULT } from "../documents/viewer";
import { StoredDocumentPicker } from "../pickers";
import { exportArtifact, previewArtifact, showArtifactPage } from "./api";
import {
  exportDoneMessage,
  exportFailureMessage,
  isBusyRefusal,
  isIdUsable,
  messageText,
  previewFailureMessage,
  summaryText,
  type UiMessage,
} from "./artifact";

/** The operation that holds the artifacts workspace. Only one runs at a time (the Rust guard too). */
type Busy = "preview" | "page" | "export" | null;

/** Props of the stored-PDF viewer. */
export type ArtifactPreviewProps = {
  /**
   * The stored PDF the viewer is fixed to (the Generation output). When given, the viewer loads it on
   * mount and shows no ID field or View button.
   */
  documentId?: string;
  /** Whether the viewer is admitted. When false, the ID field and the buttons are disabled. */
  available: boolean;
  /** Whether the Save control is rendered here. The export is admitted only when the runtime says so. */
  save: boolean;
  /**
   * The prefix of this viewer's `data-testid` values (`<prefix>Workspace`, `<prefix>Message`, ...). The
   * default is the artifacts screen's; the Generation view passes its own, so both can be mounted together.
   */
  testIdPrefix?: string;
};

/**
 * The stored-PDF viewer and export. React holds the typed ID, the page on
 * screen and the status; Rust reads, renders and publishes. The stored PDF is referenced by its ID
 * only, and the page is shown from the `jobniac-page` URL that Rust returns. The viewer is cleared on
 * every failed preview or page turn; a failed or cancelled export keeps it.
 */
export function ArtifactPreview({
  documentId,
  available,
  save,
  testIdPrefix = "artifact",
}: ArtifactPreviewProps) {
  const fixed = documentId !== undefined;
  const uid = useId();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState<Busy>(null);
  const [page, setPage] = useState<PageImageRefDto | null>(null);
  const [summary, setSummary] = useState<ArtifactSummaryDto | null>(null);
  const [message, setMessage] = useState<UiMessage | null>(null);
  const [imageFailed, setImageFailed] = useState(false);
  const [zoom, setZoom] = useState(ZOOM_DEFAULT);
  // A ref, not only the state, so that two clicks before the next render cannot both start a request.
  const inFlight = useRef(false);

  const idUsable = fixed || isIdUsable(typed);
  const locked = !available || busy !== null;

  /** Loads the first page of a stored PDF. It depends on no state, so it is created once. */
  const view = useCallback(async (target: string) => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setBusy("preview");
    try {
      const loaded = await previewArtifact(target);
      setPage(loaded.page);
      setSummary(loaded.summary);
      setImageFailed(false);
      setMessage({ id: "artifacts.preview.ready" });
    } catch (error) {
      // A busy refusal means another viewer holds the Rust guard and nothing changed: keep what is shown.
      if (isBusyRefusal(error)) {
        // No stale outcome of an earlier request may stay beside a refusal that changed nothing.
        setMessage(null);
      } else {
        setPage(null);
        setSummary(null);
        setImageFailed(false);
        setMessage(previewFailureMessage(error));
      }
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }, []);

  // A fixed viewer loads its document when it appears (the Generation output it was opened for).
  useEffect(() => {
    if (documentId !== undefined && available) {
      void view(documentId);
    }
  }, [documentId, available, view]);

  async function goToPage(index: number) {
    if (page === null || inFlight.current) {
      return;
    }
    const target = clampPageIndex(index, page.pageCount);
    if (target === page.pageIndex) {
      return;
    }
    inFlight.current = true;
    setBusy("page");
    setImageFailed(false);
    try {
      const turned = await showArtifactPage(page.documentId, target);
      if (turned.documentId !== page.documentId) {
        // The Rust viewer session is shared: another viewer replaced it, so this page is not ours.
        setPage(null);
        setSummary(null);
        setMessage({ id: "artifacts.preview.failed" });
      } else {
        setPage(turned);
      }
    } catch (error) {
      // A busy refusal changed nothing and keeps the page; a real failure clears the viewer and its pager.
      if (isBusyRefusal(error)) {
        setMessage(null);
      } else {
        setPage(null);
        setSummary(null);
        setMessage(previewFailureMessage(error));
      }
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  async function saveDocument() {
    if (inFlight.current || !idUsable) {
      return;
    }
    const target = fixed ? documentId : typed.trim();
    inFlight.current = true;
    setBusy("export");
    setMessage(null);
    try {
      const exported = await exportArtifact(target);
      setMessage(exportDoneMessage(exported));
    } catch (error) {
      setMessage(exportFailureMessage(error));
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  const progress: UiMessage | null =
    busy === "preview"
      ? { id: "artifacts.progress.preview" }
      : busy === "export"
        ? { id: "artifacts.progress.export" }
        : null;

  const retry =
    fixed && page === null && message !== null && busy === null ? (
      // The shared Rust session may have been replaced or the load failed: let the fixed viewer reload.
      <Button
        type="button"
        data-testid="retryArtifactButton"
        disabled={locked}
        onClick={() => void view(documentId)}
      >
        {formatMessage("artifacts.button.view")}
      </Button>
    ) : null;

  const saveButton = save ? (
    <Button
      type="button"
      variant={fixed ? "default" : "outline"}
      data-testid="saveArtifactButton"
      disabled={locked || !idUsable}
      onClick={() => void saveDocument()}
    >
      <Download strokeWidth={1.75} aria-hidden />
      {formatMessage("artifacts.button.save")}
    </Button>
  ) : null;

  // One live region that is always mounted, so progress and outcome texts are announced.
  const status = (
    <div
      role="status"
      aria-live="polite"
      data-testid={`${testIdPrefix}Status`}
      aria-label={progress !== null ? formatMessage("artifacts.busy") : undefined}
      className="text-[13px] empty:hidden"
    >
      {progress !== null ? (
        <span className="flex items-center gap-2">
          <Spinner />
          {messageText(progress)}
        </span>
      ) : busy === null && message !== null ? (
        <p
          data-testid={`${testIdPrefix}Message`}
          className={cn(
            "flex items-start gap-2",
            message.id === "artifacts.preview.ready" || message.id === "artifacts.export.done"
              ? "text-success"
              : "text-destructive",
          )}
        >
          {message.id === "artifacts.preview.ready" || message.id === "artifacts.export.done" ? (
            <CircleCheck className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          ) : (
            <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          )}
          <span>{messageText(message)}</span>
        </p>
      ) : null}
    </div>
  );

  const heading =
    page !== null && summary !== null ? (
      <div
        data-testid={`${testIdPrefix}Summary`}
        className="grid gap-1"
        title={summaryText(summary)}
      >
        <p className="text-base font-semibold [overflow-wrap:anywhere]">{summary.displayName}</p>
        <DocumentMeta pageCount={summary.pageCount} byteSize={summary.byteSize} />
      </div>
    ) : null;

  const viewer =
    page !== null || busy === "page" ? (
      <PageViewer
        testIdPrefix={testIdPrefix}
        page={page}
        pending={busy === "page" ? "page" : null}
        disabled={locked}
        zoom={zoom}
        imageFailed={imageFailed}
        onPage={(index) => void goToPage(index)}
        onZoom={(next) => setZoom(clampZoom(next))}
        onImageError={() => setImageFailed(true)}
      />
    ) : null;

  if (fixed) {
    return (
      <div className="grid min-w-0 gap-3" data-testid={`${testIdPrefix}Workspace`}>
        {heading}
        {retry !== null || saveButton !== null ? (
          <div className="flex flex-wrap gap-2">
            {retry}
            {saveButton}
          </div>
        ) : null}
        {status}
        {viewer}
      </div>
    );
  }

  return (
    <div
      className="grid min-w-0 items-start gap-4 @[720px]:grid-cols-[minmax(0,340px)_minmax(0,1fr)]"
      data-testid={`${testIdPrefix}Workspace`}
    >
      <div className="grid min-w-0 gap-3 rounded-panel border border-border bg-card p-4">
        <h3 className="text-base font-semibold">{formatMessage("artifacts.panel.title")}</h3>
        <Field label={formatMessage("artifacts.id.label")} id={`${uid}-document-id`}>
          {(control) => (
            <StoredDocumentPicker
              id={control.id}
              testId={`${testIdPrefix}DocumentId`}
              value={typed}
              onChange={setTyped}
              disabled={locked}
            />
          )}
        </Field>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            data-testid="previewArtifactButton"
            disabled={locked || !idUsable}
            onClick={() => void view(typed.trim())}
          >
            {formatMessage("artifacts.button.view")}
          </Button>
          {saveButton}
        </div>
        {status}
      </div>

      <div className="grid min-w-0 gap-3">
        {heading}
        {viewer ?? (
          <EmptyState
            icon={FileText}
            title={formatMessage("artifacts.empty.title")}
            text={formatMessage("artifacts.empty.text")}
          />
        )}
      </div>
    </div>
  );
}
