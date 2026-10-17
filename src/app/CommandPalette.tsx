import { Briefcase, UserRound } from "lucide-react";
import { Dialog } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { isUuid } from "../features/pickers/EntityPicker";
import { useJobOptions } from "../features/pickers/JobPicker";
import { useProfileOptions } from "../features/pickers/ProfilePicker";
import { formatMessage } from "../i18n/messages";
import { useDebounced } from "../lib/useDebounced";
import { Combobox, type ComboboxOption } from "../ui/combobox";
import { Kbd } from "../ui/kbd";
import { requestOpen } from "./openRequests";
import { SECTIONS, type SectionId } from "./sections";
import { isPaletteKey, mayRunGlobalShortcut } from "./shortcuts";

const SEARCH_DEBOUNCE_MS = 200;
const RESULT_LIMIT = 6;

/**
 * Ctrl/Cmd+K asks to open the palette. It follows the same rules as Ctrl+1..9: not while the user
 * types in a field, not while a dialog is open. (Closing it again is handled inside the palette,
 * whose own input is a field and whose dialog is open.)
 */
export function useCommandPaletteShortcut(open: () => void): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isPaletteKey(event) || !mayRunGlobalShortcut(event)) {
        return;
      }
      event.preventDefault();
      open();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open]);
}

/** Sections whose name or description contains every word of the text. */
export function matchSections(text: string) {
  const words = text.toLowerCase().split(/\s+/).filter(Boolean);
  return SECTIONS.filter((section) => {
    const haystack = `${section.label} ${section.description ?? ""}`.toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}

type Choice = { kind: "section"; id: SectionId } | { kind: "profile" | "job"; id: string };

function parseChoice(value: string): Choice | null {
  const split = value.indexOf(":");
  const kind = value.slice(0, split);
  const id = value.slice(split + 1);
  if (kind === "section" || kind === "profile" || kind === "job") {
    return { kind, id } as Choice;
  }
  return null;
}

function PaletteBody({
  select,
  onChosen,
  onToggleKey,
}: {
  select: (id: SectionId) => void;
  onChosen: () => void;
  onToggleKey: () => void;
}) {
  const [text, setText] = useState("");
  const trimmed = text.trim();
  const query = useDebounced(trimmed, SEARCH_DEBOUNCE_MS);
  const searching = trimmed !== "" && !isUuid(trimmed);
  const profiles = useProfileOptions(query, searching && query !== "");
  const jobs = useJobOptions(query, searching && query !== "");
  const pending = searching && (trimmed !== query || profiles.fetching || jobs.fetching);

  const options = useMemo<ComboboxOption[]>(() => {
    const list: ComboboxOption[] = matchSections(trimmed).map((section) => ({
      value: `section:${section.id}`,
      label: section.label,
      ...(section.description ? { description: section.description } : {}),
      icon: <section.icon className="size-4" strokeWidth={1.75} />,
      group: formatMessage("palette.groupSections"),
    }));
    if (searching && query === trimmed) {
      for (const entity of profiles.entities.slice(0, RESULT_LIMIT)) {
        list.push({
          value: `profile:${entity.id}`,
          label: entity.label,
          ...(entity.description ? { description: entity.description } : {}),
          icon: <UserRound className="size-4" strokeWidth={1.75} />,
          group: formatMessage("palette.groupProfiles"),
        });
      }
      for (const entity of jobs.entities.slice(0, RESULT_LIMIT)) {
        list.push({
          value: `job:${entity.id}`,
          label: entity.label,
          ...(entity.description ? { description: entity.description } : {}),
          icon: <Briefcase className="size-4" strokeWidth={1.75} />,
          group: formatMessage("palette.groupJobs"),
        });
      }
    }
    return list;
  }, [trimmed, searching, query, profiles.entities, jobs.entities]);

  const failures = [
    searching && query === trimmed && profiles.failed
      ? formatMessage("palette.profilesFailed")
      : "",
    searching && query === trimmed && jobs.failed ? formatMessage("palette.jobsFailed") : "",
  ].filter((message) => message !== "");

  const choose = (option: ComboboxOption) => {
    const choice = parseChoice(option.value);
    if (choice === null) {
      return;
    }
    if (choice.kind === "section") {
      select(choice.id);
    } else {
      // Ask first: a section that is already mounted opens it at once, one that mounts on this
      // navigation finds the request waiting (src/app/openRequests.ts).
      requestOpen({ kind: choice.kind, id: choice.id });
      select(choice.kind === "profile" ? "profiles" : "jobs");
    }
    onChosen();
  };

  return (
    <Combobox
      id="command-palette-input"
      inline
      autoFocus
      autoActivateFirst
      aria-label={formatMessage("palette.title")}
      placeholder={formatMessage("palette.placeholder")}
      inputValue={text}
      onInputValueChange={setText}
      options={options}
      onSelect={choose}
      loading={pending}
      emptyText={formatMessage("palette.noMatch", { query: trimmed })}
      onInputKeyDown={(event) => {
        if (isPaletteKey(event.nativeEvent)) {
          event.preventDefault();
          onToggleKey();
        }
      }}
      notice={
        failures.length > 0 ? (
          <div className="border-t border-border px-3 py-2 text-destructive">
            {failures.map((message) => (
              <p key={message}>{message}</p>
            ))}
          </div>
        ) : undefined
      }
    />
  );
}

/**
 * The Ctrl/Cmd+K command palette: a dialog with a search field over the sections and, as the user
 * types, the saved profiles and jobs. A section is opened with the `?section=` mechanism; a profile
 * or job also asks its section to open that record (`requestOpen`).
 */
export function CommandPalette({
  open,
  onOpenChange,
  select,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  select: (id: SectionId) => void;
}) {
  // After a choice the section takes the focus (HomePage); the dialog must not give it back.
  const chosen = useRef(false);
  useEffect(() => {
    if (open) {
      chosen.current = false;
    }
  }, [open]);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content
          data-testid="commandPalette"
          aria-describedby="command-palette-description"
          onCloseAutoFocus={(event) => {
            if (chosen.current) {
              event.preventDefault();
            }
          }}
          className="fixed top-[10vh] left-1/2 z-50 grid w-[min(560px,calc(100vw-32px))] -translate-x-1/2 gap-2 rounded-panel border border-border bg-popover p-2 text-popover-foreground outline-none"
        >
          <Dialog.Title className="sr-only">{formatMessage("palette.title")}</Dialog.Title>
          <Dialog.Description id="command-palette-description" className="sr-only">
            {formatMessage("palette.description")}
          </Dialog.Description>
          <PaletteBody
            select={select}
            onChosen={() => {
              chosen.current = true;
              onOpenChange(false);
            }}
            onToggleKey={() => onOpenChange(false)}
          />
          <p className="hidden items-center gap-1.5 border-t border-border px-2 pt-2 text-[13px] text-muted-foreground min-[600px]:flex">
            <Kbd>Enter</Kbd>
            <span>{formatMessage("palette.hintOpen")}</span>
            <Kbd className="ml-2">Esc</Kbd>
            <span>{formatMessage("palette.hintClose")}</span>
          </p>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
