import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * The start of a section: the page title (the one h1, focus lands on it after a section change), a
 * one-line description of what the user can do here and an optional slot for actions.
 */
function PageHeader({
  title,
  description,
  actions,
  headingRef,
  className,
  ...props
}: Omit<React.ComponentProps<"header">, "title"> & {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  headingRef?: React.Ref<HTMLHeadingElement>;
}) {
  return (
    <header
      data-slot="page-header"
      className={cn("flex flex-wrap items-end justify-between gap-x-4 gap-y-2", className)}
      {...props}
    >
      <div className="grid min-w-0 gap-1">
        <h1
          ref={headingRef}
          tabIndex={-1}
          className="text-2xl leading-tight font-semibold tracking-[-0.01em] [overflow-wrap:anywhere] outline-none"
        >
          {title}
        </h1>
        {description ? <p className="text-muted-foreground">{description}</p> : null}
      </div>
      {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
    </header>
  );
}

export { PageHeader };
