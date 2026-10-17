import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { JobDetailDto, JobPageDto } from "../../ipc/bindings";
import { JobsPage } from "./JobsPage";

// Synthetic identifiers and postings only.
const JOB_ID = "00000000-0000-4000-8000-0000000000c1";

const JOB: JobDetailDto = {
  job: {
    id: JOB_ID,
    revision: "7",
    title: "Synthetic Engineer",
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

const PAGE: JobPageDto = {
  jobs: [{ id: JOB_ID, title: "Synthetic Engineer", company: "Example Labs", location: "Remote" }],
  nextCursor: null,
};

type Overrides = Record<string, (payload: unknown) => unknown>;

/** A backend for the job screens. Unknown commands fail, so a stray call is visible. */
function backend(capability: boolean, overrides: Overrides = {}) {
  const calls: Array<{ cmd: string; payload: unknown }> = [];
  mockIPC((cmd, payload) => {
    calls.push({ cmd, payload });
    const override = overrides[cmd];
    if (override) {
      return override(payload) as never;
    }
    if (cmd === "deletion_capabilities_get") {
      return { deletion: capability } as never;
    }
    if (cmd === "list_jobs") {
      return PAGE as never;
    }
    if (cmd === "get_job") {
      return JOB as never;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  return calls;
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <JobsPage />
    </QueryClientProvider>,
  );
}

/** Lists the saved jobs and opens the one posting, then waits for its editor. */
async function openJob() {
  fireEvent.click(screen.getByTestId("refreshJobsButton"));
  fireEvent.click(await screen.findByRole("button", { name: "Open saved job Synthetic Engineer" }));
  expect(await screen.findByTestId("jobEditorPanel")).toBeInTheDocument();
}

async function openDeleteDialog() {
  fireEvent.click(await screen.findByTestId("deleteJobButton"));
  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
}

describe("job delete", () => {
  it("hides the delete control when the deletion service is not composed", async () => {
    const calls = backend(false);
    renderPage();
    await openJob();
    await waitFor(() =>
      expect(calls.some(({ cmd }) => cmd === "deletion_capabilities_get")).toBe(true),
    );
    await waitFor(() => expect(screen.queryByTestId("deleteJobButton")).toBeNull());
  });

  it("names the job, then deletes it with the echoed token and drops it from the list", async () => {
    const calls = backend(true, {
      request_job_delete: () => ({ confirmationToken: "token-9" }),
      delete_job: () => ({ jobId: JOB_ID, deleted: true }),
    });
    renderPage();
    await openJob();
    await openDeleteDialog();
    expect(screen.getByTestId("deleteJobDialogMessage").textContent).toContain(
      'Delete job "Synthetic Engineer"? This permanently removes the job',
    );

    fireEvent.click(screen.getByTestId("deleteJobDialogConfirmButton"));
    expect(await screen.findByText("Job deleted.")).toBeInTheDocument();
    expect(calls.filter(({ cmd }) => cmd === "delete_job").map(({ payload }) => payload)).toEqual([
      { jobId: JOB_ID, confirmationToken: "token-9" },
    ]);
    expect(screen.queryByTestId("jobEditorPanel")).toBeNull();
    expect(screen.queryByTestId("jobDetailPanel")).toBeNull();
    expect(screen.queryByRole("button", { name: "Open saved job Synthetic Engineer" })).toBeNull();
  });

  it("Cancel and Escape leave the job in place and never call the delete", async () => {
    const calls = backend(true, {
      request_job_delete: () => ({ confirmationToken: "token-9" }),
    });
    renderPage();
    await openJob();
    await openDeleteDialog();
    fireEvent.click(screen.getByTestId("deleteJobDialogCancelButton"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

    await openDeleteDialog();
    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

    expect(calls.some(({ cmd }) => cmd === "delete_job")).toBe(false);
    expect(screen.getByTestId("jobEditorPanel")).toBeInTheDocument();
  });

  it("says that unsaved edits will be discarded", async () => {
    backend(true, { request_job_delete: () => ({ confirmationToken: "token-9" }) });
    renderPage();
    await openJob();
    fireEvent.change(screen.getByTestId("jobEdittitleInput"), {
      target: { value: "Synthetic Lead" },
    });
    await openDeleteDialog();
    expect(screen.getByTestId("deleteJobDialogMessage").textContent).toContain(
      "Your unsaved edits to this job will be discarded.",
    );
  });

  it("a job that no longer exists leaves the list with its own copy", async () => {
    backend(true, {
      request_job_delete: () => ({ confirmationToken: "token-9" }),
      delete_job: () => {
        throw { code: "not_found", messageId: "job.deleted.missing" };
      },
    });
    renderPage();
    await openJob();
    await openDeleteDialog();
    fireEvent.click(screen.getByTestId("deleteJobDialogConfirmButton"));
    expect(
      await screen.findByText("This job no longer exists. Nothing was deleted."),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("jobEditorPanel")).toBeNull();
  });

  it("shows the fixed failure copy and keeps the editor when the delete fails", async () => {
    backend(true, {
      request_job_delete: () => ({ confirmationToken: "token-9" }),
      delete_job: () => {
        throw { code: "validation_error", messageId: "delete.failed.job" };
      },
    });
    renderPage();
    await openJob();
    await openDeleteDialog();
    fireEvent.click(screen.getByTestId("deleteJobDialogConfirmButton"));
    expect(
      await screen.findByText(
        "Could not confirm the deletion of this job. Reload to check whether it was removed.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByTestId("jobEditorPanel")).toBeInTheDocument();
  });

  it("shows the generation copy for a conflict", async () => {
    backend(true, {
      request_job_delete: () => ({ confirmationToken: "token-9" }),
      delete_job: () => {
        throw { code: "conflict", messageId: "delete.failed.conflict" };
      },
    });
    renderPage();
    await openJob();
    await openDeleteDialog();
    fireEvent.click(screen.getByTestId("deleteJobDialogConfirmButton"));
    expect(
      await screen.findByText("A generation is in progress. Try again later."),
    ).toBeInTheDocument();
  });

  it("reopens the same confirmation and drops the progress copy when the call is refused as busy", async () => {
    const tokens: unknown[] = [];
    backend(true, {
      request_job_delete: () => ({ confirmationToken: "token-9" }),
      delete_job: (payload) => {
        tokens.push((payload as { confirmationToken: unknown }).confirmationToken);
        throw { code: "busy", messageId: "job.busy" };
      },
    });
    renderPage();
    await openJob();
    await openDeleteDialog();
    const message = screen.getByTestId("deleteJobDialogMessage").textContent;

    fireEvent.click(screen.getByTestId("deleteJobDialogConfirmButton"));
    await waitFor(() => expect(tokens).toHaveLength(1));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(screen.getByTestId("deleteJobDialogMessage").textContent).toBe(message);
    expect(screen.queryByText("Deleting job…")).toBeNull();
    expect(screen.queryByText(/Could not confirm the deletion/)).toBeNull();
    expect(screen.getByTestId("jobEditorPanel")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("deleteJobDialogConfirmButton"));
    await waitFor(() => expect(tokens).toEqual(["token-9", "token-9"]));
  });
});
