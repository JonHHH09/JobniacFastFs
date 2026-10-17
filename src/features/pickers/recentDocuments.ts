import { useSyncExternalStore } from "react";

/**
 * The document picker puts the documents this window has used on top of the stored-document
 * listing: documents that were imported, generated, previewed or pasted by id. The list lives in UI
 * state only (nothing is written to disk) and keeps the newest first.
 */
export type RecentDocument = {
  id: string;
  displayName: string;
  byteSize: number;
  /** Epoch milliseconds of the last use. */
  usedAt: number;
};

const LIMIT = 12;

let recent: readonly RecentDocument[] = [];
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) {
    listener();
  }
}

/** Remembers a stored document (call it after an import, a generation or a metadata lookup). */
export function rememberDocument(
  document: { id: string; displayName: string; byteSize: number },
  usedAt: number = Date.now(),
): void {
  recent = [{ ...document, usedAt }, ...recent.filter((entry) => entry.id !== document.id)].slice(
    0,
    LIMIT,
  );
  emit();
}

/** The remembered documents, newest first. */
export function recentDocuments(): readonly RecentDocument[] {
  return recent;
}

/** Forgets every remembered document. For tests. */
export function clearRecentDocuments(): void {
  recent = [];
  emit();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The remembered documents as React state. */
export function useRecentDocuments(): readonly RecentDocument[] {
  return useSyncExternalStore(subscribe, recentDocuments, recentDocuments);
}
