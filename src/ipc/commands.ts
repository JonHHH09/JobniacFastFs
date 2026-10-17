import type { CommandError } from "./bindings";

/** The typed outcome of a generated command (the shape `typedError` returns). */
export type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

/**
 * Used when the IPC call fails and no typed error arrives. The empty message id is not a catalogue
 * id, so each screen shows its own operation copy.
 */
export const unknownFailure: CommandError = {
  code: "internal_error",
  messageId: "",
};

/** True only for the typed error shape the Rust commands return (`{ code, messageId }`). */
export function isCommandError(value: unknown): value is CommandError {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return typeof candidate.code === "string" && typeof candidate.messageId === "string";
}

/**
 * Runs one generated command and returns its data. Rejects with a `CommandError` only; any other
 * rejection (a raw string, an Error or a malformed value) becomes the generic unknown failure, so
 * raw error text from the runtime never reaches the UI.
 */
export async function runCommand<T>(call: () => Promise<CommandResult<T>>): Promise<T> {
  let result: CommandResult<T>;
  try {
    result = await call();
  } catch {
    throw unknownFailure;
  }
  if (typeof result !== "object" || result === null) {
    throw unknownFailure;
  }
  if (result.status === "error") {
    throw isCommandError(result.error) ? result.error : unknownFailure;
  }
  return result.data;
}

/** Serializes a request for the commands that take JSON text (see the Rust docs). */
export function requestText(request: unknown): string {
  return JSON.stringify(request);
}
