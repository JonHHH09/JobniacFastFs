/** True when the key press happens where the user types, so a shortcut must not steal it. */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return (
    target.isContentEditable ||
    target.contentEditable === "true" ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)
  );
}

/** True while a modal dialog (a confirmation, the navigation sheet, the palette) is open. */
export function isDialogOpen(): boolean {
  return document.querySelector('[role="dialog"], [role="alertdialog"]') !== null;
}

/**
 * Whether a global shortcut may act on this key press: not while the user types in a field and not
 * while a dialog is open, so a shortcut never changes the page under an open confirmation.
 */
export function mayRunGlobalShortcut(event: KeyboardEvent): boolean {
  return !(isTypingTarget(event.target) || isDialogOpen());
}

/** Ctrl/Cmd+K, without Alt or Shift. */
export function isPaletteKey(event: KeyboardEvent): boolean {
  return (
    (event.ctrlKey || event.metaKey) &&
    !event.altKey &&
    !event.shiftKey &&
    event.key.toLowerCase() === "k"
  );
}

/** True on macOS and iOS, where the shortcut is written Cmd+K. */
export function isApplePlatform(): boolean {
  if (typeof navigator === "undefined") {
    return false;
  }
  return /Mac|iPhone|iPad|iPod/i.test(navigator.platform || navigator.userAgent);
}

/** The keys of the palette shortcut as key-cap labels. */
export function paletteShortcutKeys(): readonly string[] {
  return isApplePlatform() ? ["⌘", "K"] : ["Ctrl", "K"];
}
