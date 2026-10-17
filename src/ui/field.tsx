import { CircleAlert } from "lucide-react";
import * as React from "react";
import { cn } from "@/lib/utils";
import { Label } from "./label";

type FieldControlProps = {
  id: string;
  "aria-describedby"?: string;
  "aria-invalid"?: true;
};

/**
 * A form field: label above the control, an optional hint and an optional error. The control is a
 * render prop that receives the id, `aria-describedby` and `aria-invalid` to spread on it.
 */
function Field({
  label,
  hint,
  error,
  id,
  className,
  children,
}: {
  label: React.ReactNode;
  hint?: React.ReactNode;
  error?: React.ReactNode;
  id?: string;
  className?: string;
  children: (props: FieldControlProps) => React.ReactNode;
}) {
  const generated = React.useId();
  const controlId = id ?? generated;
  const hintId = `${controlId}-hint`;
  const errorId = `${controlId}-error`;
  const describedBy = [hint ? hintId : null, error ? errorId : null].filter(Boolean).join(" ");
  return (
    <div data-slot="field" className={cn("grid min-w-0 gap-1.5", className)}>
      <Label htmlFor={controlId}>{label}</Label>
      {children({
        id: controlId,
        ...(describedBy ? { "aria-describedby": describedBy } : {}),
        ...(error ? { "aria-invalid": true as const } : {}),
      })}
      {hint ? (
        <p id={hintId} className="text-[13px] text-muted-foreground [overflow-wrap:anywhere]">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p
          id={errorId}
          className="flex items-start gap-1.5 text-[13px] text-destructive [overflow-wrap:anywhere]"
        >
          <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          <span>{error}</span>
        </p>
      ) : null}
    </div>
  );
}

export { Field, type FieldControlProps };
