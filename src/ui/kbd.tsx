import type * as React from "react";
import { cn } from "@/lib/utils";

/** A key cap for a keyboard shortcut. */
function Kbd({ className, ...props }: React.ComponentProps<"kbd">) {
  return (
    <kbd
      data-slot="kbd"
      className={cn(
        "inline-flex h-5 min-w-5 items-center justify-center rounded-sm border border-border bg-surface-raised px-1 font-sans text-xs leading-none font-medium text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

export { Kbd };
