import { Monitor, Moon, Sun } from "lucide-react";
import { useState } from "react";
import { copy } from "../i18n/copy";
import { cn } from "../lib/utils";
import {
  applyThemePreference,
  readThemePreference,
  THEME_PREFERENCES,
  type ThemePreference,
  writeThemePreference,
} from "./theme";

const ICONS = { system: Monitor, light: Sun, dark: Moon } as const;

/**
 * The System / Light / Dark switch of the sidebar footer. A choice is applied at once and remembered;
 * `compact` shows the icons only (the rail and the narrow sheet keep the names as accessible labels).
 */
export function ThemeSwitch({ compact = false }: { compact?: boolean }) {
  const [preference, setPreference] = useState<ThemePreference>(readThemePreference);
  const choose = (next: ThemePreference) => {
    setPreference(next);
    applyThemePreference(next);
    writeThemePreference(next);
  };
  return (
    <fieldset
      data-testid="themeSwitch"
      className={cn(
        "m-0 flex min-w-0 gap-0.5 rounded-md border border-border bg-muted p-0.5",
        compact && "flex-col",
      )}
    >
      <legend className="sr-only">{copy.theme.label}</legend>
      {THEME_PREFERENCES.map((value) => {
        const Icon = ICONS[value];
        const active = preference === value;
        return (
          <button
            key={value}
            type="button"
            aria-pressed={active}
            aria-label={compact ? copy.theme[value] : undefined}
            title={compact ? copy.theme[value] : undefined}
            data-testid={`theme-${value}`}
            onClick={() => choose(value)}
            className={cn(
              "inline-flex h-7 min-w-7 flex-1 items-center justify-center gap-1 rounded-sm px-1 text-[13px] font-medium text-muted-foreground transition-colors hover:text-foreground",
              active && "bg-card text-foreground",
            )}
          >
            <Icon className="size-4 shrink-0" strokeWidth={1.75} aria-hidden />
            {compact ? null : <span>{copy.theme[value]}</span>}
          </button>
        );
      })}
    </fieldset>
  );
}
