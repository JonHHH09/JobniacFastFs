import { formatMessage, type MessageId } from "../../i18n/messages";
import { isCommandError } from "../../ipc/commands";

/**
 * The fixed wording of the profile and job delete confirmations. Pure: no state and no IO. The entity label is user text; it
 * is inserted verbatim and rendered as plain text, never as markup.
 */

/** The entity kind of a delete. */
export type DeleteKind = "profile" | "job";

const SUBJECT_IDS: Record<DeleteKind, MessageId> = {
  profile: "delete.subject.profile",
  job: "delete.subject.job",
};

const CONFIRM_IDS: Record<DeleteKind, MessageId> = {
  profile: "delete.confirm.profile",
  job: "delete.confirm.job",
};

const UNSAVED_IDS: Record<DeleteKind, MessageId> = {
  profile: "delete.confirm.unsaved.profile",
  job: "delete.confirm.unsaved.job",
};

const FAILED_IDS: Record<DeleteKind, MessageId> = {
  profile: "delete.failed.profile",
  job: "delete.failed.job",
};

/**
 * The confirmation text for one delete. A non-empty label names the entity in double quotes; an empty
 * label falls back to `this profile` or `this job`. Unsaved edits are said to be discarded.
 */
export function confirmationMessage(
  kind: DeleteKind,
  label: string,
  hasUnsavedEdits: boolean,
): string {
  const trimmed = label.trim();
  const subject =
    trimmed === ""
      ? formatMessage(SUBJECT_IDS[kind])
      : formatMessage("delete.subject.named", { label: trimmed });
  const message = formatMessage(CONFIRM_IDS[kind], { subject });
  return hasUnsavedEdits ? `${message}${formatMessage(UNSAVED_IDS[kind])}` : message;
}

/**
 * The copy id of a failed delete. A generation in progress has its own copy; every other failure
 * shows the same fixed text, which never repeats service text and never claims nothing changed. A
 * refused call (`busy`) has no copy: nothing ran, and the screen keeps its state.
 */
export function deleteFailureId(kind: DeleteKind, error: unknown): MessageId | null {
  const code = isCommandError(error) ? error.code : "internal_error";
  if (code === "busy") {
    return null;
  }
  if (code === "conflict") {
    return "delete.failed.conflict";
  }
  return FAILED_IDS[kind];
}

/**
 * True when Rust refused the call because the delete workspace is busy. Rust keeps the confirmation
 * token for a busy refusal, so the caller reopens the same confirmation with its previous message
 * (the dialog and the message stay as they were).
 */
export function isBusyRefusal(error: unknown): boolean {
  return isCommandError(error) && error.code === "busy";
}

/** True when a failed delete says the job no longer exists. */
export function isNotFound(error: unknown): boolean {
  return isCommandError(error) && error.code === "not_found";
}
