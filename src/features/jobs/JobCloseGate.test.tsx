import { QueryClient } from "@tanstack/react-query";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { JobDetailDto } from "../../ipc/bindings";
import { JobWorkspace } from "./workspace";

// Synthetic identifiers and text only.
const JOB_ID = "00000000-0000-4000-8000-0000000000c1";

function detail(revision: string, title: string): JobDetailDto {
  return {
    job: {
      id: JOB_ID,
      revision,
      title,
      company: "Example Labs",
      location: "Remote",
      description: "Synthetic description of the role.",
      sourceLabel: "",
      employmentType: "",
      seniority: "",
      experienceRequirement: "",
      postedAt: "2026-01-02T03:04:05Z",
      sourceMethod: "text",
    },
    skills: ["Rust"],
  };
}

function newClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

/**
 * A store over a fresh client with the given commands. Every `shell_set_dirty` report is recorded,
 * in order, so the tests can read the dirty flag the Rust close gate would have received.
 */
function storeWith(handler: (cmd: string, payload: unknown) => unknown): {
  store: JobWorkspace;
  reports: Array<{ workspace: string; dirty: boolean }>;
} {
  const reports: Array<{ workspace: string; dirty: boolean }> = [];
  mockIPC((cmd, payload) => {
    if (cmd === "shell_set_dirty") {
      reports.push(payload as { workspace: string; dirty: boolean });
      return undefined;
    }
    return handler(cmd, payload);
  });
  return { store: new JobWorkspace(newClient()), reports };
}

/** Loads the job into the store and waits until its editor shows it. */
async function loadJob(store: JobWorkspace): Promise<void> {
  store.openJob(JOB_ID);
  await waitFor(() => expect(store.getSnapshot().current).not.toBeNull());
}

describe("job editor dirty reports for the window close gate", () => {
  afterEach(() => {
    clearMocks();
  });

  it("reports the draft dirty on an edit and clean again after a discard", async () => {
    const { store, reports } = storeWith((cmd) => {
      if (cmd === "get_job") {
        return detail("7", "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    await loadJob(store);
    expect(reports).toEqual([]);

    store.setField("title", "Synthetic Engineer, edited");
    expect(reports).toEqual([{ workspace: "job", dirty: true }]);

    store.discardChanges();
    expect(reports).toEqual([
      { workspace: "job", dirty: true },
      { workspace: "job", dirty: false },
    ]);
  });

  it("reports clean after a save puts the saved values in the baseline", async () => {
    const { store, reports } = storeWith((cmd) => {
      if (cmd === "get_job") {
        return detail("7", "Synthetic Engineer");
      }
      if (cmd === "update_job") {
        return detail("8", "Synthetic Engineer, edited");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    await loadJob(store);

    store.setField("title", "Synthetic Engineer, edited");
    store.saveChanges();

    await waitFor(() => expect(store.getSnapshot().editorMessage?.id).toBe("job.updated"));
    expect(reports).toEqual([
      { workspace: "job", dirty: true },
      { workspace: "job", dirty: false },
    ]);
  });

  it("reports clean when the page unmounts with edits, and dirty again when it mounts with them", async () => {
    const { store, reports } = storeWith((cmd) => {
      if (cmd === "get_job") {
        return detail("7", "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    store.attach();
    await loadJob(store);
    store.setField("title", "Synthetic Engineer, edited");

    store.detach();
    expect(reports[reports.length - 1]).toEqual({ workspace: "job", dirty: false });

    // A later mount over the same store reports the edits that are still held.
    store.attach();
    expect(reports[reports.length - 1]).toEqual({ workspace: "job", dirty: true });
  });

  it("does not report a dirty flag twice for the same state", async () => {
    const { store, reports } = storeWith((cmd) => {
      if (cmd === "get_job") {
        return detail("7", "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    await loadJob(store);
    store.setField("title", "First edit");
    store.setField("title", "Second edit");
    expect(reports).toEqual([{ workspace: "job", dirty: true }]);
  });
});
