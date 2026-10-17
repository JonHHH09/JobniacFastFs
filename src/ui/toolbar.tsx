import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * The top toolbar of a section: the start slot (search, filters) grows, the end slot holds the
 * actions. Both wrap onto their own line at narrow widths.
 */
function Toolbar({
  end,
  className,
  children,
  ...props
}: React.ComponentProps<"div"> & { end?: React.ReactNode }) {
  return (
    <div
      data-slot="toolbar"
      className={cn("flex flex-wrap items-center justify-between gap-x-3 gap-y-2", className)}
      {...props}
    >
      <div className="flex min-w-0 flex-1 basis-64 flex-wrap items-center gap-2">{children}</div>
      {end ? <div className="flex flex-wrap items-center gap-2">{end}</div> : null}
    </div>
  );
}

export { Toolbar };
