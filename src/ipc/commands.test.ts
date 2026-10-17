import { describe, expect, it } from "vitest";
import type { CommandError } from "./bindings";
import { type CommandResult, runCommand, unknownFailure } from "./commands";

const typed: CommandError = { code: "validation_failed", messageId: "job.validation.unsaved" };

async function rejection(call: () => Promise<unknown>): Promise<unknown> {
  try {
    await runCommand(call as () => Promise<CommandResult<unknown>>);
  } catch (error) {
    return error;
  }
  throw new Error("runCommand resolved");
}

describe("runCommand", () => {
  it("returns the data of an ok result", async () => {
    await expect(runCommand(() => Promise.resolve({ status: "ok", data: 42 }))).resolves.toBe(42);
  });

  it("passes a typed command error through unchanged", async () => {
    const error = await rejection(() => Promise.resolve({ status: "error", error: typed }));
    expect(error).toBe(typed);
  });

  it("turns a string rejection into the unknown failure", async () => {
    expect(await rejection(() => Promise.reject("boom"))).toBe(unknownFailure);
  });

  it("never lets the text of a rejected Error reach the caller", async () => {
    const error = await rejection(() => Promise.reject(new Error("secret path /x")));
    expect(error).toBe(unknownFailure);
    expect(JSON.stringify(error)).not.toContain("secret");
  });

  it("replaces a malformed error result with the unknown failure", async () => {
    const error = await rejection(() => Promise.resolve({ status: "error", error: { foo: 1 } }));
    expect(error).toBe(unknownFailure);
  });

  it("replaces a null or non-object result with the unknown failure", async () => {
    expect(await rejection(() => Promise.resolve(null))).toBe(unknownFailure);
    expect(await rejection(() => Promise.resolve(undefined))).toBe(unknownFailure);
  });
});
