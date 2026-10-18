import { describe, expect, it } from "vitest";
import settingsSource from "../../src-tauri/src/settings_commands.rs?raw";
import stateSource from "../../src-tauri/src/state.rs?raw";
import updateSource from "../../src-tauri/src/update_commands.rs?raw";
import { formatMessage, isMessageId } from "./messages";

/**
 * Message ids the Rust core emits as `pub const NAME: &str = "id"` in `src-tauri/src/state.rs` and
 * `src-tauri/src/settings_commands.rs` and `src-tauri/src/update_commands.rs`.
 * Every one must have catalogue copy, except the refusals below that carry no copy on purpose
 *: each screen either filters by `isMessageId` or shows its
 * own operation text, and the close gate only shows ids that are in the catalogue.
 */
const NO_COPY_ON_PURPOSE = new Set([
  "arbeitnow.busy",
  "health.busy",
  "job.busy",
  "profile.busy",
  "profiles.busy",
  "matching.busy",
  "reviews.busy",
  "settings.busy",
]);

function rustMessageIds(): string[] {
  const ids = [
    ...(stateSource + settingsSource + updateSource).matchAll(
      /pub const [A-Z0-9_]+: &str = "([a-zA-Z0-9.]+)";/g,
    ),
  ].map((match) => match[1] as string);
  return [...new Set(ids)];
}

describe("message id catalogue", () => {
  it("has copy for every id Rust emits, except the documented no-copy refusals", () => {
    const ids = rustMessageIds();
    expect(ids.length).toBeGreaterThan(10);
    const missing = ids.filter((id) => !(isMessageId(id) || NO_COPY_ON_PURPOSE.has(id)));
    expect(missing).toEqual([]);
  });

  it("does not list a no-copy id that now has copy or is no longer emitted", () => {
    const emitted = new Set(rustMessageIds());
    for (const id of NO_COPY_ON_PURPOSE) {
      expect(isMessageId(id), `${id} has copy, drop it from the allow-list`).toBe(false);
    }
    const stale = [...NO_COPY_ON_PURPOSE].filter((id) => !emitted.has(id));
    expect(stale).toEqual([]);
  });

  it("recognises own catalogue keys only, never inherited object members", () => {
    expect(isMessageId("job.busy.close")).toBe(true);
    expect(isMessageId("constructor")).toBe(false);
    expect(isMessageId("toString")).toBe(false);
    expect(isMessageId("__proto__")).toBe(false);
  });

  it("formats a placeholder from own params only", () => {
    const id = "profileSearch.summary.none";
    expect(formatMessage(id, { query: "rust" })).toBe("No profiles matched “rust”.");
    expect(formatMessage(id, Object.create({ query: "inherited" }))).toBe(
      "No profiles matched “{query}”.",
    );
  });
});
