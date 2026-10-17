import { ChevronRight } from "lucide-react";
import type * as React from "react";
import { cn } from "@/lib/utils";

type ListRowProps = {
  title: React.ReactNode;
  meta?: React.ReactNode;
  trailing?: React.ReactNode;
  selected?: boolean;
  /** Makes the row a `<button>` (with a trailing chevron); without it the row is a plain `<div>`. */
  onSelect?: () => void;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
  "data-testid"?: string;
};

const ROW =
  "relative flex min-h-12 w-full min-w-0 items-center gap-3 px-4 py-2 text-left outline-none transition-colors before:absolute before:inset-y-0 before:left-0 before:w-[3px] before:bg-marker before:opacity-0 aria-[current=true]:bg-marker/20 aria-[current=true]:before:opacity-100";
const INTERACTIVE =
  "cursor-pointer hover:bg-muted focus-visible:bg-muted focus-visible:ring-3 focus-visible:ring-ring focus-visible:ring-inset disabled:cursor-default disabled:opacity-60";

/**
 * A row of a list: title (14px/600), a graphite meta line below, an optional trailing slot. The
 * selected row has `aria-current="true"` and a marker bar on its left edge.
 */
function ListRow({
  title,
  meta,
  trailing,
  selected = false,
  onSelect,
  className,
  disabled,
  "aria-label": ariaLabel,
  "data-testid": testId,
}: ListRowProps) {
  const content = (
    <>
      <span className="grid min-w-0 flex-1 gap-0.5">
        <span className="text-sm leading-snug font-semibold [overflow-wrap:anywhere]">{title}</span>
        {meta ? (
          <span className="text-[13px] leading-snug text-muted-foreground [overflow-wrap:anywhere]">
            {meta}
          </span>
        ) : null}
      </span>
      {trailing ? <span className="flex shrink-0 items-center gap-2">{trailing}</span> : null}
      {onSelect ? (
        <ChevronRight
          className="size-4 shrink-0 text-muted-foreground"
          strokeWidth={1.75}
          aria-hidden
        />
      ) : null}
    </>
  );
  const current = selected ? ("true" as const) : undefined;
  if (onSelect) {
    return (
      <button
        type="button"
        data-slot="list-row"
        data-testid={testId}
        aria-label={ariaLabel}
        aria-current={current}
        disabled={disabled}
        onClick={onSelect}
        className={cn(ROW, INTERACTIVE, className)}
      >
        {content}
      </button>
    );
  }
  return (
    <div
      data-slot="list-row"
      data-testid={testId}
      aria-current={current}
      className={cn(ROW, className)}
    >
      {content}
    </div>
  );
}

export { ListRow };
