import { cva, type VariantProps } from "class-variance-authority";
import type * as React from "react";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs leading-none font-medium whitespace-nowrap [&_svg]:size-3",
  {
    variants: {
      variant: {
        neutral: "border-transparent bg-muted text-foreground",
        outline: "border-border bg-transparent text-foreground",
        success: "border-success/40 bg-transparent text-success",
        warning: "border-warning/40 bg-transparent text-warning",
        destructive: "border-destructive/40 bg-transparent text-destructive",
        marker: "marker-inline border-transparent",
      },
    },
    defaultVariants: { variant: "neutral" },
  },
);

/** A short label for a state or a count. The text always names the state, colour only supports it. */
function Badge({
  className,
  variant,
  ...props
}: React.ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return (
    <span
      data-slot="badge"
      data-variant={variant ?? "neutral"}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  );
}

export { Badge, badgeVariants };
