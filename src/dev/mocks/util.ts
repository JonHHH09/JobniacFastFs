// Shared helpers of the development mock IPC. Everything here is synthetic and dev-only.

/** The arguments of one command call, as `invoke` sends them. */
export type Args = Record<string, unknown>;

/** One command handler: the decoded arguments in, the command's data out (or a thrown error). */
export type Handler = (args: Args) => unknown | Promise<unknown>;

/** The handlers of one feature, keyed by the Rust command name (snake_case). */
export type HandlerMap = Record<string, Handler>;

/** The query string of the page, read once; the stores below are seeded while modules load. */
export const mockParams = new URLSearchParams(
  typeof window === "undefined" ? "" : window.location.search,
);

/** Run-time switches read from the URL. */
export const mockConfig = {
  /** True when the URL carries `?mockEmpty=1`: every store starts with no rows. */
  empty: mockParams.get("mockEmpty") === "1",
};

/** Throws the typed error a Rust command returns (`{ code, messageId }`), as a plain object. */
export function fail(code: string, messageId: string): never {
  throw { code, messageId };
}

/** The error of `?mockFail=<command>`: an upstream failure with no catalogue copy. */
export function injectedFailure(): never {
  return fail("upstream_unavailable", "mock.failure");
}

/** Decodes the `requestJson` argument of the commands that take JSON text. */
export function requestOf(args: Args): Record<string, unknown> {
  return JSON.parse(String(args.requestJson ?? "{}")) as Record<string, unknown>;
}

/** A deterministic, valid UUID text: `kind` groups the entity type, `n` is its running number. */
export function uuid(kind: number, n: number): string {
  return `00000000-0000-4000-8000-${kind.toString(16).padStart(4, "0")}${n
    .toString(16)
    .padStart(8, "0")}`;
}

/** A fresh random UUID for rows created while the mock runs. */
export function freshId(): string {
  return crypto.randomUUID();
}

/** The current instant as ISO text, for `createdAt` / `updatedAt` of new rows. */
export function nowIso(): string {
  return new Date().toISOString();
}

/** The base instant of the fixtures, so the lists look the same on every load. */
export const FIXTURE_TIME = "2026-03-14T09:30:00Z";

/** Returns `rows` unless `?mockEmpty=1` is set. */
export function seed<T>(rows: T[]): T[] {
  return mockConfig.empty ? [] : rows;
}

/** One cursor page: the cursor is the decimal offset of the first row. */
export function pageOf<T>(
  rows: readonly T[],
  cursor: unknown,
  size: number,
): { items: T[]; nextCursor: string | null } {
  const start = typeof cursor === "string" && /^\d+$/.test(cursor) ? Number(cursor) : 0;
  const end = start + size;
  return { items: rows.slice(start, end), nextCursor: end < rows.length ? String(end) : null };
}

/** The decimal string after `revision` (revisions cross IPC as strings). */
export function nextRevision(revision: string): string {
  return String(Number(revision) + 1);
}

/** Case-insensitive search tokens of a query. */
export function tokensOf(query: string): string[] {
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter((token) => token.length > 0);
}
