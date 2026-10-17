import type * as React from "react";
import { cn } from "@/lib/utils";

/**
 * A polite live region that stays mounted. Screen readers announce a change of its text reliably,
 * but not a region that is inserted together with its text. While the text is empty the node is
 * visually removed (sr-only, not display:none, so it stays in the accessibility tree).
 */
export function LiveStatus({
  text,
  icon,
  className,
  ...props
}: Omit<React.ComponentProps<"p">, "children"> & { text: string; icon?: React.ReactNode }) {
  return (
    <p role="status" aria-live="polite" className={cn("empty:sr-only", className)} {...props}>
      {icon && text !== "" ? icon : null}
      {text}
    </p>
  );
}
