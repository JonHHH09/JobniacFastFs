import { RadioGroup } from "radix-ui";
import type * as React from "react";
import { cn } from "@/lib/utils";

type SegmentedOption = { value: string; label: React.ReactNode; disabled?: boolean };

/**
 * A segmented choice of one value (zoom, language...), built on a radio group: arrow keys move and
 * select, Tab enters and leaves the group. Give it an `aria-label` or `aria-labelledby`.
 */
function SegmentedControl({
  options,
  className,
  ...props
}: Omit<React.ComponentProps<typeof RadioGroup.Root>, "children" | "orientation"> & {
  options: readonly SegmentedOption[];
}) {
  return (
    <RadioGroup.Root
      data-slot="segmented-control"
      orientation="horizontal"
      className={cn(
        "inline-flex min-w-0 gap-0.5 rounded-md border border-border bg-muted p-0.5",
        className,
      )}
      {...props}
    >
      {options.map((option) => (
        <RadioGroup.Item
          key={option.value}
          value={option.value}
          disabled={option.disabled}
          data-slot="segmented-control-item"
          className="inline-flex h-7 min-w-8 items-center justify-center rounded-sm px-2.5 text-[13px] font-medium text-muted-foreground transition-colors outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 data-[state=checked]:bg-card data-[state=checked]:text-foreground"
        >
          {option.label}
        </RadioGroup.Item>
      ))}
    </RadioGroup.Root>
  );
}

export { SegmentedControl, type SegmentedOption };
