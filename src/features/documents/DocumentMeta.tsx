import { formatMessage } from "../../i18n/messages";
import { formatBytes } from "../../lib/format";

/** The meta line of a PDF header: the page count (when known) and the size, as separate elements. */
export function DocumentMeta({
  pageCount,
  byteSize,
}: {
  pageCount?: number | null;
  byteSize: number | string;
}) {
  return (
    <p className="flex flex-wrap items-center gap-x-2 text-[13px] text-muted-foreground tabular-nums">
      {pageCount !== undefined && pageCount !== null ? (
        <>
          <span>
            {pageCount === 1
              ? formatMessage("artifacts.meta.page")
              : formatMessage("artifacts.meta.pages", { pageCount })}
          </span>
          <span aria-hidden className="h-3 w-px bg-border" />
        </>
      ) : null}
      <span>{formatBytes(byteSize)}</span>
    </p>
  );
}
