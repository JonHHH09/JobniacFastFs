import { ChevronLeft, ChevronRight, CircleAlert } from "lucide-react";
import { useId } from "react";
import { formatMessage } from "../../i18n/messages";
import type { PageImageRefDto } from "../../ipc/bindings";
import { Button } from "../../ui/button";
import { SegmentedControl } from "../../ui/segmented-control";
import { Spinner } from "../../ui/spinner";
import { canGoToPage, clampZoom, ZOOM_MAX, ZOOM_MIN, ZOOM_STEP } from "./viewer";

/** The zoom levels the segmented control offers, from the smallest to the largest, one step apart. */
const ZOOM_LEVELS = Array.from(
  { length: Math.round((ZOOM_MAX - ZOOM_MIN) / ZOOM_STEP) + 1 },
  (_, index) => ZOOM_MIN + index * ZOOM_STEP,
);

/** Props of the page-image viewer. The parent owns the requests; this component only shows and asks. */
export type PageViewerProps = {
  /**
   * The prefix of the viewer's `data-testid` values (`<prefix>-viewer`, `<prefix>-pager-label`, ...). The
   * default keeps the documents viewer's ids; another viewer on the same screen passes its own prefix.
   */
  testIdPrefix?: string;
  /** The page on screen, or `null` when nothing is loaded (empty state). */
  page: PageImageRefDto | null;
  /** The viewer's own request in flight: the first page (`view`) or a page turn (`page`). */
  pending: "view" | "page" | null;
  /** Whether the viewer controls are disabled by the screen (another action runs, or no service). */
  disabled: boolean;
  /** The zoom in effect (a UI scale; the raster is never upscaled). */
  zoom: number;
  /** Whether the image element failed to load the current page URL. */
  imageFailed: boolean;
  /** Asks for the given zero-based page; the parent clamps it again before it sends the request. */
  onPage: (pageIndex: number) => void;
  /** Asks for a new zoom level. */
  onZoom: (zoom: number) => void;
  /** Reports that the browser could not show the page image. */
  onImageError: () => void;
};

/**
 * The page-image viewer. React displays the raster from the
 * `jobniac-page` URL that Rust returned; it never builds a URL, reads a file or sees bytes. Paging is
 * bounded by the page count and zoom by its range, as UX hints only. A compact toolbar holds the pager
 * and the zoom; the page lies on a darker desk surface with the one soft paper shadow of the app.
 */
export function PageViewer({
  testIdPrefix = "document",
  page,
  pending,
  disabled,
  zoom,
  imageFailed,
  onPage,
  onZoom,
  onImageError,
}: PageViewerProps) {
  const controlsDisabled = disabled || pending !== null;
  // Element ids are unique per mounted viewer: the documents and the artifact viewers can be on screen together.
  const uid = useId();

  return (
    <section
      aria-label={formatMessage("documents.viewer.a11y")}
      className="min-w-0 overflow-hidden rounded-panel border border-border bg-card"
      data-testid={`${testIdPrefix}-viewer`}
    >
      {page !== null ? (
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 border-b border-border px-3 py-2">
          <div className="flex items-center gap-1">
            <Button
              type="button"
              id={`${uid}-previous-page`}
              variant="ghost"
              size="icon-sm"
              aria-label={formatMessage("documents.pager.previous")}
              title={formatMessage("documents.pager.previous")}
              disabled={controlsDisabled || !canGoToPage(page.pageIndex - 1, page.pageCount)}
              onClick={() => onPage(page.pageIndex - 1)}
            >
              <ChevronLeft strokeWidth={1.75} aria-hidden />
            </Button>
            <p
              className="min-w-14 text-center text-[13px] font-medium tabular-nums"
              data-testid={`${testIdPrefix}-pager-label`}
            >
              <span className="sr-only">{formatMessage("documents.pager.prefix")} </span>
              {formatMessage("documents.pager.label", {
                page: page.pageIndex + 1,
                total: page.pageCount,
              })}
            </p>
            <Button
              type="button"
              id={`${uid}-next-page`}
              variant="ghost"
              size="icon-sm"
              aria-label={formatMessage("documents.pager.next")}
              title={formatMessage("documents.pager.next")}
              disabled={controlsDisabled || !canGoToPage(page.pageIndex + 1, page.pageCount)}
              onClick={() => onPage(page.pageIndex + 1)}
            >
              <ChevronRight strokeWidth={1.75} aria-hidden />
            </Button>
          </div>
          <SegmentedControl
            aria-label={formatMessage("documents.zoom.group")}
            data-testid={`${testIdPrefix}-zoom`}
            value={String(clampZoom(zoom))}
            disabled={controlsDisabled}
            onValueChange={(next) => onZoom(Number(next))}
            options={ZOOM_LEVELS.map((level) => ({
              value: String(level),
              label: `${Math.round(level * 100)}%`,
            }))}
          />
        </div>
      ) : null}
      {pending !== null ? (
        <p role="status" className="flex items-center gap-2 px-3 py-2 text-[13px]">
          <Spinner />
          {formatMessage("documents.progress.page")}
        </p>
      ) : null}
      {page === null ? (
        <p
          className="px-4 py-8 text-center text-muted-foreground"
          data-testid={`${testIdPrefix}-viewer-empty`}
        >
          {formatMessage("documents.viewer.empty")}
        </p>
      ) : imageFailed ? (
        <p
          className="flex items-center gap-2 px-4 py-6 text-destructive"
          role="alert"
          data-testid={`${testIdPrefix}-viewer-error`}
        >
          <CircleAlert className="size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          {formatMessage("documents.viewer.imageError")}
        </p>
      ) : (
        <div className="max-h-[70vh] overflow-auto bg-muted p-3 @[720px]:p-5">
          <img
            src={page.url}
            alt={formatMessage("documents.image.alt")}
            width={page.width}
            loading="eager"
            onError={onImageError}
            data-testid={`${testIdPrefix}-viewer-image`}
            className="mx-auto bg-white shadow-[0_1px_2px_rgb(0_0_0/0.14),0_8px_24px_rgb(0_0_0/0.12)]"
            style={{
              display: "block",
              width: `${Math.round(zoom * 100)}%`,
              maxWidth: "none",
              height: "auto",
              aspectRatio: `${page.width} / ${page.height}`,
            }}
          />
        </div>
      )}
    </section>
  );
}
