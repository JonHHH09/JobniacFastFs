import type { LucideIcon } from "lucide-react";
import type * as React from "react";
import { cn } from "@/lib/utils";

/** An empty screen is an invitation to act: an icon, what is missing, why, and the next step. */
function EmptyState({
  icon: Icon,
  title,
  text,
  action,
  className,
  ...props
}: Omit<React.ComponentProps<"div">, "title"> & {
  icon?: LucideIcon;
  title: string;
  text?: string;
  action?: React.ReactNode;
}) {
  return (
    <div
      data-slot="empty-state"
      className={cn(
        "grid justify-items-center gap-2 rounded-panel border border-dashed border-border px-6 py-8 text-center",
        className,
      )}
      {...props}
    >
      {Icon ? (
        <Icon className="size-4 text-muted-foreground" strokeWidth={1.75} aria-hidden />
      ) : null}
      <p className="text-base font-semibold">{title}</p>
      {text ? <p className="max-w-prose text-muted-foreground">{text}</p> : null}
      {action ? <div className="mt-1">{action}</div> : null}
    </div>
  );
}

export { EmptyState };
