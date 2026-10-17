import type * as React from "react";
import { cn } from "@/lib/utils";

/** A quiet placeholder block for content that is loading. It does not animate. */
function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="skeleton"
      aria-hidden="true"
      className={cn("rounded-md bg-muted", className)}
      {...props}
    />
  );
}

export { Skeleton };
