import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * A horizontal highlighter bar for a score from 0 to 100. The meter carries its value as text
 * (aria-valuenow and aria-valuetext, plus the visible number), so the bar is never the only signal.
 */
function ScoreMeter({
  value,
  label,
  showValue = true,
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "children"> & {
  value: number;
  /** The accessible name of the meter, for example "Match score". */
  label: string;
  showValue?: boolean;
}) {
  const clamped = Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : 0;
  return (
    <div data-slot="score-meter" className={cn("flex items-center gap-3", className)} {...props}>
      {/* biome-ignore lint/a11y/useSemanticElements: a native <meter> cannot take the marker styling */}
      <div
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={clamped}
        aria-valuetext={`${clamped} of 100`}
        className="h-3 min-w-0 flex-1 overflow-hidden rounded-md border border-border bg-muted"
      >
        <div
          className="marker-block h-full"
          style={{ width: `${clamped}%`, backgroundColor: "var(--marker-bar)" }}
        />
      </div>
      {showValue ? (
        <span data-tabular className="w-9 shrink-0 text-right font-semibold tabular-nums">
          {clamped}
        </span>
      ) : null}
    </div>
  );
}

export { ScoreMeter };
