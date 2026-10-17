import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * A small CSS ring for work in progress. It is decorative: the caller names the activity in text
 * (a status message or a button label). With reduced motion it stands still.
 */
function Spinner({ className, ...props }: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="spinner"
      aria-hidden="true"
      className={cn(
        "inline-block size-4 animate-spin rounded-full border-2 border-border border-t-primary motion-reduce:animate-none",
        className,
      )}
      {...props}
    />
  );
}

export { Spinner };
