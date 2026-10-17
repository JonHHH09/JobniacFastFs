import { type KeyboardEvent, useSyncExternalStore } from "react";
import { dirtyWorkspaces, subscribeDirty } from "../features/shell/closeGate";
import { copy } from "../i18n/copy";
import { cn } from "../lib/utils";
import { Kbd } from "../ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { SECTION_GROUPS, SECTIONS, type SectionDefinition, type SectionId } from "./sections";

/** The modifier shown in shortcut hints: Command on Apple platforms, Ctrl elsewhere. */
export function modifierLabel(): string {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform;
  return /mac|iphone|ipad/i.test(platform) ? "⌘" : "Ctrl";
}

function NavItem({
  section,
  active,
  dirty,
  rail,
  onSelect,
}: {
  section: SectionDefinition;
  active: boolean;
  dirty: boolean;
  rail: boolean;
  onSelect: (id: SectionId) => void;
}) {
  const Icon = section.icon;
  const position = SECTIONS.indexOf(section) + 1;
  const button = (
    <button
      type="button"
      data-nav-item=""
      data-testid={`nav-${section.id}`}
      aria-current={active ? "page" : undefined}
      onClick={() => onSelect(section.id)}
      className={cn(
        "relative isolate flex h-9 w-full items-center rounded-md px-2 text-left text-sm font-medium transition-colors",
        active
          ? "text-marker-foreground"
          : "text-muted-foreground hover:bg-muted hover:text-foreground",
        rail && "justify-center",
      )}
    >
      <span className="relative flex min-w-0 items-center gap-2.5">
        {active ? (
          <span
            aria-hidden="true"
            data-testid="navMarker"
            className="marker-block absolute -inset-x-1.5 -inset-y-1 -z-10 origin-left animate-marker-sweep motion-reduce:animate-none"
          />
        ) : null}
        <Icon className="size-4 shrink-0" strokeWidth={1.75} aria-hidden />
        <span className={cn("truncate", rail && "sr-only")}>{section.label}</span>
      </span>
      {dirty ? (
        <>
          <span
            aria-hidden="true"
            data-testid={`navDirty-${section.id}`}
            className={cn(
              "absolute size-2 rounded-full bg-warning",
              rail ? "top-1.5 right-1.5" : "top-1/2 right-2 -translate-y-1/2",
            )}
          />
          <span className="sr-only">{copy.nav.unsavedChanges}</span>
        </>
      ) : null}
    </button>
  );
  if (!rail) {
    return button;
  }
  return (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent side="right">
        {section.label}
        {position <= 9 ? <Kbd>{`${modifierLabel()} ${position}`}</Kbd> : null}
      </TooltipContent>
    </Tooltip>
  );
}

/** Arrow keys, Home and End move focus between the items of the list. */
function moveFocus(event: KeyboardEvent<HTMLElement>): void {
  const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
  if (!keys.includes(event.key)) {
    return;
  }
  const items = Array.from(
    event.currentTarget.querySelectorAll<HTMLButtonElement>("button[data-nav-item]"),
  );
  const current = items.indexOf(document.activeElement as HTMLButtonElement);
  let next = current;
  if (event.key === "ArrowDown") {
    next = current < 0 ? 0 : (current + 1) % items.length;
  } else if (event.key === "ArrowUp") {
    next = current <= 0 ? items.length - 1 : current - 1;
  } else if (event.key === "Home") {
    next = 0;
  } else {
    next = items.length - 1;
  }
  event.preventDefault();
  items[next]?.focus();
}

/**
 * The section list, grouped. `rail` shows icons only (the labels stay as accessible names and
 * tooltips). A dot marks a section whose workspace holds unsaved edits.
 */
export function SectionNav({
  active,
  onSelect,
  rail = false,
}: {
  active: SectionId;
  onSelect: (id: SectionId) => void;
  rail?: boolean;
}) {
  const dirty = useSyncExternalStore(subscribeDirty, dirtyWorkspaces, dirtyWorkspaces);
  return (
    <nav aria-label={copy.nav.main} onKeyDown={moveFocus} className="grid gap-4">
      {SECTION_GROUPS.map((group) => (
        <div key={group.id} className="grid gap-0.5">
          <p
            id={`nav-group-${group.id}`}
            className={cn("px-2 pb-1 text-[13px] text-muted-foreground", rail && "sr-only")}
          >
            {group.label}
          </p>
          <ul aria-labelledby={`nav-group-${group.id}`} className="grid gap-0.5">
            {group.sections.map((section) => (
              <li key={section.id}>
                <NavItem
                  section={section}
                  active={section.id === active}
                  dirty={
                    section.dirtyWorkspace !== undefined && dirty.includes(section.dirtyWorkspace)
                  }
                  rail={rail}
                  onSelect={onSelect}
                />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </nav>
  );
}
