import type * as React from "react";
import { cn } from "@/lib/utils";

/** A bar that sticks to the bottom of its scroll container: Save, Discard and Delete live here. */
function StickyActions({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sticky-actions"
      className={cn(
        "sticky bottom-0 z-10 flex flex-wrap items-center gap-2 border-t border-border bg-card px-4 py-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]",
        className,
      )}
      {...props}
    />
  );
}

export { StickyActions };
