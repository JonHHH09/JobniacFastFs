import type * as React from "react";
import { cn } from "@/lib/utils";

/** The highlighter: marks a word or phrase the way a printed posting is marked. Use it sparingly. */
function Marker({ className, ...props }: React.ComponentProps<"span">) {
  return <span data-slot="marker" className={cn("marker-inline", className)} {...props} />;
}

export { Marker };
