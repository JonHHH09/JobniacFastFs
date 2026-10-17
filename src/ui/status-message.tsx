import { cva, type VariantProps } from "class-variance-authority";
import { CircleAlert, CircleCheck, Info, TriangleAlert } from "lucide-react";
import type * as React from "react";
import { copy } from "@/i18n/copy";
import { cn } from "@/lib/utils";

const statusMessageVariants = cva("flex items-start gap-2 [overflow-wrap:anywhere]", {
  variants: {
    tone: {
      neutral: "text-muted-foreground",
      success: "text-success",
      error: "text-destructive",
      warning: "text-warning",
    },
  },
  defaultVariants: { tone: "neutral" },
});

type Tone = NonNullable<VariantProps<typeof statusMessageVariants>["tone"]>;

const ICONS = { neutral: Info, success: CircleCheck, error: CircleAlert, warning: TriangleAlert };

/**
 * A short outcome message in a polite live region. The tone is never conveyed by colour alone: the
 * icon differs and a screen reader hears the tone as a word before the text.
 */
function StatusMessage({
  tone = "neutral",
  className,
  children,
  ...props
}: React.ComponentProps<"p"> & { tone?: Tone }) {
  const Icon = ICONS[tone];
  return (
    <p
      role="status"
      aria-live="polite"
      data-slot="status-message"
      data-tone={tone}
      className={cn(statusMessageVariants({ tone }), className)}
      {...props}
    >
      <Icon className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
      <span>
        {tone === "neutral" ? null : <span className="sr-only">{copy.status[tone]}</span>}
        {children}
      </span>
    </p>
  );
}

export { StatusMessage, statusMessageVariants };
