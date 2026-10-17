import { commands, type ShellDirtyWorkspace } from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

/**
 * Reports whether a job or profile draft holds unsaved edits, so the Rust window close gate can
 * refuse or ask for the discard confirmation. React calls it only when the flag changes. Not guarded
 * by single flight; it touches no rows.
 */
export function setShellDirty(workspace: ShellDirtyWorkspace, dirty: boolean): Promise<null> {
  markDirty(workspace, dirty);
  return runCommand(() => commands.shellSetDirty(workspace, dirty));
}

// The same flags, mirrored for the section navigation: a nav item shows a dot while its
// workspace holds unsaved edits. Rust stays the authority for the close gate; this is display only.
let dirtySnapshot: readonly ShellDirtyWorkspace[] = [];
const dirtyListeners = new Set<() => void>();

function markDirty(workspace: ShellDirtyWorkspace, dirty: boolean): void {
  if (dirtySnapshot.includes(workspace) === dirty) {
    return;
  }
  dirtySnapshot = dirty
    ? [...dirtySnapshot, workspace]
    : dirtySnapshot.filter((entry) => entry !== workspace);
  for (const listener of dirtyListeners) {
    listener();
  }
}

/** Subscribes to changes of the dirty workspaces (for `useSyncExternalStore`). */
export function subscribeDirty(listener: () => void): () => void {
  dirtyListeners.add(listener);
  return () => {
    dirtyListeners.delete(listener);
  };
}

/** The workspaces that hold unsaved edits now. The array is replaced on every change. */
export function dirtyWorkspaces(): readonly ShellDirtyWorkspace[] {
  return dirtySnapshot;
}

/**
 * Asks for the window to close after the user discarded the unsaved profile edits. Rust runs the
 * whole gate again, so the call only closes when every other workspace allows it.
 */
export function requestShellClose(): Promise<null> {
  return runCommand(() => commands.shellRequestClose());
}
