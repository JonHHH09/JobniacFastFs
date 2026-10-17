import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { isMessageId, type MessageId } from "../i18n/messages";

/** The event the Rust window close gate emits when it refuses a close. */
export const CLOSE_REFUSED_EVENT = "shell:close-refused";

/** The reason of the refusal that opens the profile discard confirmation. */
export const CONFIRM_DISCARD_REASON = "confirm_discard";

/** The payload of a refused close. Rust decides; React only shows the copy of `messageId`. */
export type CloseRefusedPayload = {
  workspace: string;
  reason: string;
  messageId: string;
};

/** Subscribes to the refused-close event; the handler gets every payload until the cleanup runs. */
function listenToRefusals(handler: (payload: CloseRefusedPayload) => void): () => void {
  let active = true;
  let unlisten: (() => void) | undefined;
  listen<CloseRefusedPayload>(CLOSE_REFUSED_EVENT, (event) => {
    if (active) {
      handler(event.payload);
    }
  }).then(
    (stop) => {
      if (active) {
        unlisten = stop;
      } else {
        stop();
      }
    },
    () => undefined,
  );
  return () => {
    active = false;
    unlisten?.();
  };
}

/**
 * Calls `onRequest` when the Rust close gate asks for the profile discard confirmation. The handler
 * is read through a ref, so the latest editor state is used without re-subscribing.
 */
export function useCloseDiscardRequest(onRequest: () => void): void {
  const latest = useRef(onRequest);
  useEffect(() => {
    latest.current = onRequest;
  });
  useEffect(
    () =>
      listenToRefusals((payload) => {
        if (payload.reason === CONFIRM_DISCARD_REASON) {
          latest.current();
        }
      }),
    [],
  );
}

/** A refused close shown as a notice. `serial` is new for every refusal, so a repeat is a new notice. */
export type CloseRefusal = { id: MessageId; serial: number };

/**
 * The latest refused close, or null, and the way to dismiss it. Subscribes once to the Rust event; a
 * runtime without the event bridge (a browser preview) simply never receives one. Every refusal is
 * a new value (its `serial`), so an identical repeat is announced again. The discard confirmation
 * is shown by its dialog, not as a notice.
 */
export function useCloseRefusal(): { refusal: CloseRefusal | null; dismiss: () => void } {
  const [refusal, setRefusal] = useState<CloseRefusal | null>(null);
  const serial = useRef(0);
  useEffect(
    () =>
      listenToRefusals((payload) => {
        const id = payload.messageId;
        if (payload.reason !== CONFIRM_DISCARD_REASON && isMessageId(id)) {
          serial.current += 1;
          setRefusal({ id, serial: serial.current });
        }
      }),
    [],
  );
  const dismiss = useCallback(() => setRefusal(null), []);
  return { refusal, dismiss };
}
