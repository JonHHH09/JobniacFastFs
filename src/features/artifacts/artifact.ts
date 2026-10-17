import { formatMessage, type MessageId } from "../../i18n/messages";
import type { ArtifactExportDto, ArtifactSummaryDto } from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";

/** The stored document ID accepts at most 36 characters. */
export const ID_MAX_LENGTH = 36;

/** A catalogue message to show, with its placeholder values. */
export type UiMessage = { id: MessageId; params?: Record<string, string | number> };

/** The capabilities before the startup check answers: nothing is admitted. */
export const NOT_ADMITTED = { artifacts: false, export: false } as const;

/** Whether the typed ID can be sent: non-blank after trimming (Rust admits the UUID itself). */
export function isIdUsable(text: string): boolean {
  return text.trim() !== "" && text.trim().length <= ID_MAX_LENGTH;
}

/** The summary line of a loaded PDF (`artifacts.summary`). It always says "pages". */
export function summaryText(summary: ArtifactSummaryDto): string {
  return formatMessage("artifacts.summary", {
    displayName: summary.displayName,
    pageCount: summary.pageCount,
    byteSize: summary.byteSize,
  });
}

/**
 * The message of a finished export. `null` after a cancelled dialog: nothing changed, so nothing is
 * said (`artifact_export_document`).
 */
export function exportDoneMessage(exported: ArtifactExportDto | null): UiMessage | null {
  if (exported === null) {
    return null;
  }
  return { id: "artifacts.export.done", params: { exportedName: exported.exportedName } };
}

/** Whether Rust refused the request because another artifact operation holds the shared guard. */
export function isBusyRefusal(error: unknown): boolean {
  return isCommandError(error) && error.messageId === "artifacts.busy";
}

/**
 * The message of a failed export. Completion that could not be verified has its own copy; every other
 * failure shows the export copy. The busy refusal is only an accessible name, so it shows nothing.
 */
export function exportFailureMessage(error: unknown): UiMessage | null {
  const failure = isCommandError(error) ? error : unknownFailure;
  if (failure.messageId === "artifacts.busy") {
    return null;
  }
  if (failure.messageId === "artifacts.export.uncertain" || failure.code === "export_uncertain") {
    return { id: "artifacts.export.uncertain" };
  }
  if (failure.messageId === "artifacts.input.id") {
    return { id: "artifacts.input.id" };
  }
  return { id: "artifacts.export.failed" };
}

/**
 * The message of a failed preview or page turn. The viewer is cleared on every such failure. The busy
 * refusal shows nothing: the controls are locked while another request runs.
 */
export function previewFailureMessage(error: unknown): UiMessage | null {
  const failure = isCommandError(error) ? error : unknownFailure;
  if (failure.messageId === "artifacts.busy") {
    return null;
  }
  if (failure.messageId === "artifacts.input.id") {
    return { id: "artifacts.input.id" };
  }
  return { id: "artifacts.preview.failed" };
}

/** The copy of a message, with its placeholders filled in. */
export function messageText(message: UiMessage): string {
  return formatMessage(message.id, message.params);
}
