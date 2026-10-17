import { Outlet } from "@tanstack/react-router";
import { Menu, Search, X } from "lucide-react";
import { Dialog } from "radix-ui";
import { useCallback, useEffect, useState } from "react";
import { copy } from "../i18n/copy";
import { formatMessage } from "../i18n/messages";
import { Button } from "../ui/button";
import { Kbd } from "../ui/kbd";
import { TooltipProvider } from "../ui/tooltip";
import { CommandPalette, useCommandPaletteShortcut } from "./CommandPalette";
import { useCloseRefusal } from "./closeRefusal";
import { SectionNav } from "./SectionNav";
import { SECTIONS, type SectionId } from "./sections";
import { isDialogOpen, isTypingTarget, paletteShortcutKeys } from "./shortcuts";
import { ThemeSwitch } from "./ThemeSwitch";
import { useActiveSection } from "./useActiveSection";
import { useViewport } from "./useViewport";

/**
 * Ctrl/Cmd+1 to 9 jump to the nth section. Skipped while the user types in a field or a dialog is
 * open, so the shortcut never changes the section under an open confirmation.
 */
function useSectionShortcuts(select: (id: SectionId) => void): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) {
        return;
      }
      if (!/^[1-9]$/.test(event.key)) {
        return;
      }
      const section = SECTIONS[Number(event.key) - 1];
      if (!section || isTypingTarget(event.target)) {
        return;
      }
      if (isDialogOpen()) {
        return;
      }
      event.preventDefault();
      select(section.id);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [select]);
}

function Brand({ rail = false }: { rail?: boolean }) {
  return (
    <p className="text-base font-semibold">
      {rail ? (
        <>
          <span aria-hidden="true" className="marker-inline px-1.5">
            J
          </span>
          <span className="sr-only">{copy.appName}</span>
        </>
      ) : (
        copy.appName
      )}
    </p>
  );
}

/** The sidebar footer's way into the palette: a button with the shortcut written on it. */
function PaletteButton({ onOpen, compact }: { onOpen: () => void; compact: boolean }) {
  const label = formatMessage("palette.openAria");
  return (
    <Button
      type="button"
      variant="outline"
      size={compact ? "icon" : "default"}
      aria-label={label}
      aria-keyshortcuts="Control+K Meta+K"
      title={compact ? label : undefined}
      data-testid="paletteButton"
      onClick={onOpen}
      className={compact ? "mx-auto" : "w-full justify-start text-muted-foreground"}
    >
      <Search strokeWidth={1.75} aria-hidden />
      {compact ? null : (
        <>
          <span className="flex-1 text-left">{formatMessage("palette.open")}</span>
          <span className="flex items-center gap-1" aria-hidden="true">
            {paletteShortcutKeys().map((key) => (
              <Kbd key={key}>{key}</Kbd>
            ))}
          </span>
        </>
      )}
    </Button>
  );
}

/** The sidebar of the wide layout and the icon rail of the middle layout. */
function Sidebar({
  active,
  select,
  rail,
  onOpenPalette,
}: {
  active: SectionId;
  select: (id: SectionId) => void;
  rail: boolean;
  onOpenPalette: () => void;
}) {
  return (
    <aside
      data-testid="sidebar"
      className={`sticky top-0 flex h-screen shrink-0 flex-col overflow-hidden border-r border-border bg-card ${
        rail ? "w-16" : "w-[232px]"
      }`}
    >
      <div className={rail ? "flex shrink-0 justify-center px-2 py-4" : "shrink-0 px-4 py-4"}>
        <Brand rail={rail} />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        <SectionNav active={active} onSelect={select} rail={rail} />
      </div>
      <div className="grid shrink-0 gap-2 border-t border-border p-2">
        <PaletteButton onOpen={onOpenPalette} compact={rail} />
        <ThemeSwitch compact={rail} />
      </div>
    </aside>
  );
}

/** The top bar of the narrow layout: the name and the menu button that opens the navigation sheet. */
function TopBar({
  active,
  select,
  onOpenPalette,
}: {
  active: SectionId;
  select: (id: SectionId) => void;
  onOpenPalette: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <header className="sticky top-0 z-20 flex h-12 items-center justify-between border-b border-border bg-card px-4">
        <Brand />
        <div className="flex items-center gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={formatMessage("palette.openAria")}
            data-testid="paletteButton"
            onClick={onOpenPalette}
          >
            <Search strokeWidth={1.75} aria-hidden />
          </Button>
          <Dialog.Trigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={copy.nav.openMenu}
              data-testid="menuButton"
            >
              <Menu strokeWidth={1.75} aria-hidden />
            </Button>
          </Dialog.Trigger>
        </div>
      </header>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-foreground/40" />
        <Dialog.Content
          aria-describedby={undefined}
          className="fixed inset-y-0 left-0 z-50 flex w-[min(280px,85vw)] flex-col border-r border-border bg-popover text-popover-foreground outline-none"
        >
          <div className="flex items-center justify-between px-4 py-2">
            <Dialog.Title className="text-base font-semibold">{copy.nav.menuTitle}</Dialog.Title>
            <Dialog.Close asChild>
              <Button type="button" variant="ghost" size="icon" aria-label={copy.nav.closeMenu}>
                <X strokeWidth={1.75} aria-hidden />
              </Button>
            </Dialog.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
            <SectionNav
              active={active}
              onSelect={(id) => {
                setOpen(false);
                select(id);
              }}
            />
          </div>
          <div className="border-t border-border p-2">
            <ThemeSwitch />
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The window shell: a left sidebar (icon rail on a middle width, a top bar with a menu sheet on a
 * narrow one) and the content area. The active section is the `?section=` search param. A
 * close the Rust gate refused is shown above the content as the copy of its message id.
 */
export function AppShell() {
  const { refusal: closeNotice, dismiss: dismissCloseNotice } = useCloseRefusal();
  const { active, select } = useActiveSection();
  const viewport = useViewport();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const openPalette = useCallback(() => setPaletteOpen(true), []);
  useSectionShortcuts(select);
  useCommandPaletteShortcut(openPalette);
  // Moving to another section is the next action on the page: the stale refusal goes away.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `active` is the trigger, not a read value
  useEffect(() => dismissCloseNotice(), [active, dismissCloseNotice]);
  return (
    <TooltipProvider>
      <div className="min-h-screen min-w-0 min-[600px]:flex">
        <a
          className="sr-only top-2 left-2 z-50 rounded-md border border-border bg-card px-3 py-2 text-card-foreground focus:not-sr-only focus:absolute"
          href="#main-content"
        >
          {copy.nav.skipToContent}
        </a>
        {viewport === "compact" ? (
          <TopBar active={active} select={select} onOpenPalette={openPalette} />
        ) : (
          <Sidebar
            active={active}
            select={select}
            rail={viewport === "rail"}
            onOpenPalette={openPalette}
          />
        )}
        <main
          id="main-content"
          tabIndex={-1}
          className="grid min-w-0 flex-1 content-start gap-[18px] px-4 py-6 focus:outline-none min-[600px]:px-10 min-[600px]:py-8"
        >
          {closeNotice !== null ? (
            <div
              key={closeNotice.serial}
              role="alert"
              data-testid="closeRefusedNotice"
              className="flex w-full max-w-[1200px] items-center justify-between gap-3 text-muted-foreground"
            >
              <p>{formatMessage(closeNotice.id)}</p>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label={copy.shell.dismissNotice}
                onClick={dismissCloseNotice}
              >
                <X strokeWidth={1.75} aria-hidden />
              </Button>
            </div>
          ) : null}
          <Outlet />
        </main>
        <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} select={select} />
      </div>
    </TooltipProvider>
  );
}
