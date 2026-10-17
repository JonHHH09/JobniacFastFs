import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  JobCaptureDto,
  JobDetailDto,
  JobPageDto,
  JobSearchDto,
  JobSummaryDto,
} from "../../ipc/bindings";
import { JobsPage } from "./JobsPage";
import { JobWorkspace } from "./workspace";

// Synthetic identifiers and text only.
const JOB_ID = "00000000-0000-4000-8000-0000000000c1";
const OTHER_ID = "00000000-0000-4000-8000-0000000000c2";

function summary(id: string, title: string): JobSummaryDto {
  return { id, title, company: "Example Labs", location: "Remote" };
}

function detail(id: string, title: string, revision = "7", skills = ["Rust"]): JobDetailDto {
  return {
    job: {
      id,
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
    skills,
  };
}

const firstPage: JobPageDto = {
  jobs: [summary(JOB_ID, "Synthetic Engineer"), summary(OTHER_ID, "Synthetic Analyst")],
  nextCursor: "cursor-2",
};

const secondPage: JobPageDto = {
  jobs: [summary("00000000-0000-4000-8000-0000000000c3", "Synthetic Designer")],
  nextCursor: null,
};

const searchResult: JobSearchDto = {
  query: "rust",
  totalMatches: "3",
  matchedCount: "3",
  hasMore: false,
  returnedCount: 2,
  jobs: [
    { job: summary(JOB_ID, "Synthetic Engineer") },
    { job: summary(OTHER_ID, "Synthetic Analyst") },
  ],
};

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/** A promise the test settles by hand, to hold an operation open. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  let fail: (reason: unknown) => void = () => undefined;
  const promise = new Promise<T>((done, failed) => {
    resolve = done;
    fail = failed;
  });
  return { promise, resolve, fail };
}

function newClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

function renderPage() {
  return render(
    <QueryClientProvider client={newClient()}>
      <JobsPage />
    </QueryClientProvider>,
  );
}

function statusOf(testId: string): string {
  return screen.getByTestId(testId).textContent ?? "";
}

function typeInto(testId: string, value: string) {
  fireEvent.change(screen.getByTestId(testId), { target: { value } });
}

function openRow(title: string) {
  const button = screen.getByRole("button", { name: `Open saved job ${title}` });
  fireEvent.click(button);
}

/** The calls that read or write job data. The link capability (`job_link_capabilities_get`) is excluded. */
function jobDataCalls(invoke: { mock: { calls: unknown[][] } }): unknown[][] {
  return invoke.mock.calls.filter(([cmd]) => cmd !== "job_link_capabilities_get");
}

describe("job paste", () => {
  it("does not load at startup and refuses a blank posting before any request", () => {
    const invoke = vi.fn(() => firstPage);
    mockIPC(invoke);

    renderPage();

    // The link capability is a session question, not job data: only job commands count here.
    expect(jobDataCalls(invoke)).toEqual([]);
    expect(screen.getByTestId("addJobButton")).toBeDisabled();
    expect(statusOf("jobEmptyLabel")).toBe("No saved jobs loaded. Refresh to check for postings.");
  });

  it("saves a posting, shows the saved status, clears the input and refreshes the list", async () => {
    const capture: JobCaptureDto = {
      status: "created_job",
      job: detail(JOB_ID, "Synthetic Engineer"),
    };
    const seen: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "add_job_from_text") {
        seen.push(payload);
        return capture;
      }
      if (cmd === "list_jobs") {
        return { jobs: [summary(JOB_ID, "Synthetic Engineer")], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobTextInput", "Synthetic posting text");
    fireEvent.click(screen.getByTestId("addJobButton"));

    expect(await screen.findByText("Job posting saved.")).toBeInTheDocument();
    expect(seen).toEqual([{ requestJson: JSON.stringify({ text: "Synthetic posting text" }) }]);
    expect(screen.getByTestId("jobTextInput")).toHaveValue("");
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
  });

  it("says when the posting was already saved", async () => {
    mockIPC((cmd) => {
      if (cmd === "add_job_from_text") {
        return { status: "reused_existing_job", job: detail(JOB_ID, "Synthetic Engineer") };
      }
      if (cmd === "list_jobs") {
        return firstPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobTextInput", "Synthetic posting text");
    fireEvent.click(screen.getByTestId("addJobButton"));

    expect(await screen.findByText("This posting was already saved.")).toBeInTheDocument();
  });

  it("keeps the text and shows the save copy when the save fails", async () => {
    mockIPC((cmd) => {
      if (cmd === "add_job_from_text") {
        reject("internal_error", "job.error.add");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobTextInput", "Synthetic posting text");
    fireEvent.click(screen.getByTestId("addJobButton"));

    expect(
      await screen.findByText("Unable to save this job posting. Please try again."),
    ).toBeInTheDocument();
    expect(screen.getByTestId("jobTextInput")).toHaveValue("Synthetic posting text");
    expect(screen.getByTestId("jobSaveStatusLabel")).toBeEmptyDOMElement();
  });

  it("shows the progress copy and holds the saved-jobs controls while a save runs", async () => {
    const save = deferred<JobCaptureDto>();
    mockIPC((cmd) => {
      if (cmd === "add_job_from_text") {
        return save.promise;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobTextInput", "Synthetic posting text");
    fireEvent.click(screen.getByTestId("addJobButton"));

    expect(await screen.findByText("Saving job posting…")).toBeInTheDocument();
    expect(screen.getByTestId("refreshJobsButton")).toBeDisabled();
    expect(screen.getByTestId("jobSearchButton")).toBeDisabled();
  });

  it("disables Save posting while a list runs, and a click sends no save", async () => {
    const list = deferred<JobPageDto>();
    const invoke = vi.fn((cmd: string) => {
      if (cmd === "list_jobs") {
        return list.promise;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    mockIPC(invoke);

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    typeInto("jobTextInput", "Synthetic posting text");

    expect(screen.getByTestId("addJobButton")).toBeDisabled();
    fireEvent.click(screen.getByTestId("addJobButton"));
    expect(invoke.mock.calls.some(([cmd]) => cmd === "add_job_from_text")).toBe(false);

    list.resolve(firstPage);
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
  });
});

describe("job service without a database", () => {
  it("shows the unconfigured copy and disables the job controls", async () => {
    const invoke = vi.fn((cmd: string) => {
      if (cmd === "list_jobs") {
        reject("not_configured", "jobs.unconfigured");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    mockIPC(invoke);

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));

    await waitFor(() => expect(statusOf("jobStatusLabel")).toBe("Job service is not configured."));
    expect(screen.getByTestId("refreshJobsButton")).toBeDisabled();
    expect(screen.getByTestId("jobSearchInput")).toBeDisabled();
    expect(screen.getByTestId("jobSearchButton")).toBeDisabled();
    expect(screen.getByTestId("jobTextInput")).toBeDisabled();
    expect(screen.getByTestId("addJobButton")).toBeDisabled();
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "list_jobs")).toHaveLength(1);
  });
});

describe("job text and editor edge cases", () => {
  it("shows the posting text as plain text, never as markup", async () => {
    const markup = "<script>window.synthetic = true</script><b>bold</b>";
    const described = detail(JOB_ID, "Synthetic Engineer");
    described.job.description = markup;
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return described;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    const { container } = renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");

    expect(await screen.findByTestId("jobDescriptionLabel")).toHaveTextContent(markup);
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
  });

  it("keeps refresh and paste blocked while the editor holds unsaved edits", async () => {
    const invoke = vi.fn((cmd: string) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    mockIPC(invoke);

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");
    typeInto("jobEdittitleInput", "Synthetic title v2");

    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(
      await screen.findByText("Save or discard your job changes before continuing."),
    ).toBeInTheDocument();
    typeInto("jobTextInput", "Synthetic posting text");
    fireEvent.click(screen.getByTestId("addJobButton"));

    expect(invoke.mock.calls.filter(([cmd]) => cmd === "list_jobs")).toHaveLength(1);
    expect(invoke.mock.calls.some(([cmd]) => cmd === "add_job_from_text")).toBe(false);
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic title v2");
  });

  it("sends an empty skills text when the skills are removed, which clears them", async () => {
    const seen: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer", "7", ["Rust", "SQL"]);
      }
      if (cmd === "update_job") {
        seen.push(payload);
        return detail(JOB_ID, "Synthetic Engineer", "8", []);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    expect(await screen.findByTestId("jobEditSkillsInput")).toHaveValue("Rust\nSQL");

    typeInto("jobEditSkillsInput", "");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(await screen.findByText("Job changes saved.")).toBeInTheDocument();
    expect(seen).toEqual([
      { jobId: JOB_ID, expectedRevision: "7", requestJson: JSON.stringify({ skillsText: "" }) },
    ]);
  });

  it("keeps the edits and shows the update copy when the save fails for another reason", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      if (cmd === "update_job") {
        reject("internal_error", "job.error.update");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");
    typeInto("jobEdittitleInput", "Synthetic title v2");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(
      await screen.findByText(
        "Unable to save job changes. Your edits are kept. Check them and try again.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic title v2");
  });

  it("keeps the edits when the save returns a malformed job", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      if (cmd === "update_job") {
        return {};
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");
    typeInto("jobEdittitleInput", "Synthetic title v2");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(
      await screen.findByText(
        "Unable to save job changes. Your edits are kept. Check them and try again.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic title v2");
  });

  it("keeps Save job changes disabled when the loaded revision is not an integer", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer", "7.5");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");
    typeInto("jobEdittitleInput", "Synthetic title v2");

    expect(screen.getByTestId("saveJobChangesButton")).toBeDisabled();
  });

  it("loads the job selected from a search result", async () => {
    mockIPC((cmd) => {
      if (cmd === "search_jobs") {
        return searchResult;
      }
      if (cmd === "get_job") {
        return detail(OTHER_ID, "Synthetic Analyst");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobSearchInput", "rust");
    fireEvent.click(screen.getByTestId("jobSearchButton"));
    expect(await screen.findByText("Synthetic Analyst")).toBeInTheDocument();

    openRow("Synthetic Analyst");
    expect(await screen.findByTestId("jobTitleLabel")).toHaveTextContent("Synthetic Analyst");
  });
});

describe("saved jobs list", () => {
  it("loads page one on refresh and appends the next page", async () => {
    const cursors: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "list_jobs") {
        cursors.push((payload as { cursor: string | null }).cursor);
        return (payload as { cursor: string | null }).cursor === null ? firstPage : secondPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("nextJobsPageButton"));
    expect(await screen.findByText("Synthetic Designer")).toBeInTheDocument();
    expect(screen.getByText("Synthetic Engineer")).toBeInTheDocument();
    expect(cursors).toEqual([null, "cursor-2"]);
    expect(screen.queryByTestId("nextJobsPageButton")).not.toBeInTheDocument();
  });

  it("keeps the rows already shown when a refresh fails", async () => {
    let calls = 0;
    mockIPC((cmd) => {
      if (cmd !== "list_jobs") {
        throw new Error(`unexpected command ${cmd}`);
      }
      calls += 1;
      if (calls === 1) {
        return firstPage;
      }
      reject("internal_error", "job.error.list");
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(
      await screen.findByText("Unable to load saved jobs. Please try again."),
    ).toBeInTheDocument();
    expect(screen.getByText("Synthetic Engineer")).toBeInTheDocument();
  });

  it("says so when the listing is empty", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return { jobs: [], nextCursor: null };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));

    expect(await screen.findByText("No saved jobs found.")).toBeInTheDocument();
    // One empty-state sentence, not the status and the hint together.
    expect(screen.queryByTestId("jobEmptyLabel")).not.toBeInTheDocument();
  });

  it("does not claim the list is empty after the first load failed", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        reject("internal_error", "job.error.list");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    expect(screen.getByTestId("jobEmptyLabel")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));

    expect(
      await screen.findByText("Unable to load saved jobs. Please try again."),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("jobEmptyLabel")).not.toBeInTheDocument();
  });

  it("shows placeholder rows while the first load runs", async () => {
    const list = deferred<JobPageDto>();
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return list.promise;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    expect(screen.queryByTestId("jobListSkeleton")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByTestId("jobListSkeleton")).toBeInTheDocument();

    list.resolve(firstPage);
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    expect(screen.queryByTestId("jobListSkeleton")).not.toBeInTheDocument();
  });

  it("renders job titles as headings", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(
      await screen.findByRole("heading", { level: 3, name: "Synthetic Engineer" }),
    ).toBeInTheDocument();

    openRow("Synthetic Engineer");
    expect(
      await screen.findByRole("heading", { level: 2, name: "Synthetic Engineer" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Synthetic Engineer" })).toBeInTheDocument();
  });
});

describe("live status regions", () => {
  it("keeps every status region mounted and empty before any message", () => {
    mockIPC(() => firstPage);

    renderPage();

    for (const id of [
      "jobStatusLabel",
      "jobPasteStatusLabel",
      "jobSaveStatusLabel",
      "jobSearchStatusLabel",
      "jobListStatusLabel",
    ]) {
      const region = screen.getByTestId(id);
      expect(region).toHaveAttribute("role", "status");
      expect(region).toHaveAttribute("aria-live", "polite");
      expect(region).toBeEmptyDOMElement();
    }
  });
});

describe("job search", () => {
  it("refuses a blank query before any request", () => {
    const invoke = vi.fn(() => firstPage);
    mockIPC(invoke);

    renderPage();
    typeInto("jobSearchInput", "   ");

    expect(screen.getByTestId("jobSearchButton")).toBeDisabled();
    expect(jobDataCalls(invoke)).toEqual([]);
  });

  it("sends the query as entered, shows the summary and returns to the list", async () => {
    const seen: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "search_jobs") {
        seen.push(payload);
        return searchResult;
      }
      if (cmd === "list_jobs") {
        return firstPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobSearchInput", "  rust ");
    fireEvent.click(screen.getByTestId("jobSearchButton"));

    expect(await screen.findByText("Showing 2 of 3 matching jobs.")).toBeInTheDocument();
    expect(seen).toEqual([{ requestJson: JSON.stringify({ query: "  rust " }) }]);
    expect(screen.getByRole("heading", { name: "Job search results" })).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("clearJobSearchButton"));
    expect(await screen.findByRole("heading", { name: "Your postings" })).toBeInTheDocument();
    expect(screen.getByTestId("jobSearchInput")).toHaveValue("");
  });

  it("says the matches may continue when the search has more", async () => {
    mockIPC((cmd) => {
      if (cmd === "search_jobs") {
        return { ...searchResult, totalMatches: null, hasMore: true };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    typeInto("jobSearchInput", "rust");
    fireEvent.click(screen.getByTestId("jobSearchButton"));

    expect(
      await screen.findByText("Showing 2 results; additional matches may exist."),
    ).toBeInTheDocument();
  });

  it("shows the search copy when the search fails and keeps the list", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "search_jobs") {
        reject("internal_error", "job.error.search");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();

    typeInto("jobSearchInput", "rust");
    fireEvent.click(screen.getByTestId("jobSearchButton"));

    expect(
      await screen.findByText(
        "Unable to search jobs. Check the query and local database, then try again.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Synthetic Engineer")).toBeInTheDocument();
  });
});

describe("job editor", () => {
  it("opens a job and sends only the changed fields under the loaded revision", async () => {
    const seen: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      if (cmd === "update_job") {
        seen.push(payload);
        return detail(JOB_ID, "Synthetic title v2", "8");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");

    expect(await screen.findByTestId("jobEditorPanel")).toBeInTheDocument();
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic Engineer");
    expect(screen.getByTestId("saveJobChangesButton")).toBeDisabled();

    typeInto("jobEdittitleInput", "Synthetic title v2");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(await screen.findByText("Job changes saved.")).toBeInTheDocument();
    expect(seen).toEqual([
      {
        jobId: JOB_ID,
        expectedRevision: "7",
        requestJson: JSON.stringify({ title: "Synthetic title v2" }),
      },
    ]);
    expect(screen.getByTestId("jobTitleLabel")).toHaveTextContent("Synthetic title v2");
  });

  it("sends the skills text only when the skills changed, one skill per line", async () => {
    const seen: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer", "7", ["Rust", "SQL"]);
      }
      if (cmd === "update_job") {
        seen.push(payload);
        return detail(JOB_ID, "Synthetic Engineer", "8", ["Go"]);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    expect(await screen.findByTestId("jobEditSkillsInput")).toHaveValue("Rust\nSQL");

    typeInto("jobEditSkillsInput", "Go\n");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(await screen.findByText("Job changes saved.")).toBeInTheDocument();
    expect(seen).toEqual([
      { jobId: JOB_ID, expectedRevision: "7", requestJson: JSON.stringify({ skillsText: "Go\n" }) },
    ]);
  });

  it("does not mark the editor dirty for a whitespace-only skills edit, but does for a real one", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer", "7", ["Rust", "SQL"]);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    expect(await screen.findByTestId("jobEditSkillsInput")).toHaveValue("Rust\nSQL");
    expect(screen.getByTestId("saveJobChangesButton")).toBeDisabled();

    typeInto("jobEditSkillsInput", "  Rust  \n\n SQL \n   ");
    expect(screen.getByTestId("saveJobChangesButton")).toBeDisabled();

    typeInto("jobEditSkillsInput", "Rust\nSQL\nGo");
    expect(screen.getByTestId("saveJobChangesButton")).toBeEnabled();
  });

  it("keeps the edits and shows the conflict copy when the job changed elsewhere", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      if (cmd === "update_job") {
        reject("conflict", "job.error.conflict");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");

    typeInto("jobEdittitleInput", "Synthetic title v2");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(
      await screen.findByText(
        "This job changed elsewhere. Your edits are kept. Reload the latest job to continue.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic title v2");
  });

  it("refuses a blank posting date before any request", async () => {
    const invoke = vi.fn((cmd: string) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    mockIPC(invoke);

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");

    typeInto("jobEditpostedAtInput", "  ");
    fireEvent.click(screen.getByTestId("saveJobChangesButton"));

    expect(
      await screen.findByText("Keep the loaded posting date or enter a new date."),
    ).toBeInTheDocument();
    expect(invoke.mock.calls.some(([cmd]) => cmd === "update_job")).toBe(false);
  });

  it("discards the edits without a confirmation and reloads the latest job", async () => {
    const seen: string[] = [];
    mockIPC((cmd) => {
      seen.push(cmd);
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        return detail(JOB_ID, "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");

    typeInto("jobEdittitleInput", "Synthetic title v2");
    fireEvent.click(screen.getByTestId("discardJobChangesButton"));
    expect(await screen.findByText("Job changes discarded.")).toBeInTheDocument();
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic Engineer");

    typeInto("jobEdittitleInput", "Synthetic title v3");
    fireEvent.click(screen.getByTestId("reloadJobButton"));
    await waitFor(() => expect(seen.filter((cmd) => cmd === "get_job")).toHaveLength(2));
    await waitFor(() =>
      expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic Engineer"),
    );
  });
});

describe("navigation rules", () => {
  it("blocks opening another job while edits are unsaved and keeps the draft", async () => {
    const opened: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        opened.push(payload);
        return detail((payload as { jobId: string }).jobId, "Synthetic Engineer");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");
    await screen.findByTestId("jobEditorPanel");

    typeInto("jobEdittitleInput", "Synthetic title v2");
    openRow("Synthetic Analyst");

    expect(
      await screen.findByText("Save or discard your job changes before continuing."),
    ).toBeInTheDocument();
    expect(opened).toHaveLength(1);
    expect(screen.getByTestId("jobEdittitleInput")).toHaveValue("Synthetic title v2");
  });

  it("shows the not-found copy when the opened job no longer exists", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_jobs") {
        return firstPage;
      }
      if (cmd === "get_job") {
        reject("not_found", "job.error.detailNotFound");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("refreshJobsButton"));
    expect(await screen.findByText("Synthetic Engineer")).toBeInTheDocument();
    openRow("Synthetic Engineer");

    const status = await screen.findByTestId("jobStatusLabel");
    expect(status).toHaveTextContent("This saved job is no longer available.");
    expect(screen.queryByTestId("jobEditorPanel")).not.toBeInTheDocument();
  });
});

describe("navigation queue (store)", () => {
  /** A store over a fresh client, driven without the page so each operation can be held open. */
  function storeWith(handler: (cmd: string, payload: unknown) => unknown) {
    mockIPC(handler);
    return new JobWorkspace(newClient());
  }

  it("clears the search summary when the saved jobs are shown again", async () => {
    const store = storeWith((cmd) => {
      if (cmd === "search_jobs") {
        return searchResult;
      }
      if (cmd === "list_jobs") {
        return firstPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    store.setSearchInput("rust");
    store.search();
    await waitFor(() => expect(store.getSnapshot().searchMessage).not.toBeNull());

    store.showAllJobs();

    expect(store.getSnapshot().searchMessage).toBeNull();
    await waitFor(() => expect(store.getSnapshot().busy).toBeNull());
    expect(store.getSnapshot().searchMessage).toBeNull();
  });

  it("applies a selection made during a save once the save ends", async () => {
    const save = deferred<JobCaptureDto>();
    const commands: string[] = [];
    const store = storeWith((cmd) => {
      commands.push(cmd);
      if (cmd === "add_job_from_text") {
        return save.promise;
      }
      if (cmd === "get_job") {
        return detail(OTHER_ID, "Synthetic Analyst");
      }
      if (cmd === "list_jobs") {
        return firstPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    store.setPasteText("Synthetic posting text");
    store.savePaste();
    store.openJob(OTHER_ID);
    expect(commands).toEqual(["add_job_from_text"]);

    save.resolve({ status: "created_job", job: detail(JOB_ID, "Synthetic Engineer") });

    await waitFor(() => expect(store.getSnapshot().current?.job.id).toBe(OTHER_ID));
    // The selection replaces the follow-up refresh: the saved-jobs list is not reloaded.
    expect(commands).toEqual(["add_job_from_text", "get_job"]);
    // A successful add with a queued navigation leaves the store idle, with the saved status kept.
    expect(store.getSnapshot().busy).toBeNull();
    expect(store.getSnapshot().saveStatus).toBe("created_job");
  });

  it("runs a search issued during a save exactly once", async () => {
    const save = deferred<JobCaptureDto>();
    const commands: string[] = [];
    const store = storeWith((cmd) => {
      commands.push(cmd);
      if (cmd === "add_job_from_text") {
        return save.promise;
      }
      if (cmd === "search_jobs") {
        return searchResult;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    store.setPasteText("Synthetic posting text");
    store.savePaste();
    store.setSearchInput("rust");
    store.search();

    save.resolve({ status: "created_job", job: detail(JOB_ID, "Synthetic Engineer") });

    await waitFor(() => expect(store.getSnapshot().searchQuery).toBe("rust"));
    expect(commands.filter((cmd) => cmd === "search_jobs")).toHaveLength(1);
    expect(store.getSnapshot().jobs).toHaveLength(2);
  });

  it("drops the result of a superseded search and runs the newest request", async () => {
    const search = deferred<JobSearchDto>();
    const commands: string[] = [];
    const store = storeWith((cmd) => {
      commands.push(cmd);
      if (cmd === "search_jobs") {
        return search.promise;
      }
      if (cmd === "get_job") {
        return detail(OTHER_ID, "Synthetic Analyst");
      }
      if (cmd === "list_jobs") {
        return firstPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    store.setSearchInput("rust");
    store.search();
    store.openJob(OTHER_ID);
    store.refresh();

    search.resolve(searchResult);

    // The superseded result is dropped: the refresh runs and its page is what the list shows.
    await waitFor(() => expect(commands).toContain("list_jobs"));
    await waitFor(() => expect(store.getSnapshot().jobs).toHaveLength(2));
    expect(store.getSnapshot().searchQuery).toBe("");
    expect(store.getSnapshot().jobs.map((job) => job.id)).toEqual([JOB_ID, OTHER_ID]);
    expect(store.getSnapshot().current).toBeNull();
    // The newest request (refresh) ran; the earlier open was replaced and never sent.
    expect(commands).toEqual(["search_jobs", "list_jobs"]);
  });

  it("ignores a superseded search that fails and shows no search message", async () => {
    const search = deferred<JobSearchDto>();
    const commands: string[] = [];
    const store = storeWith((cmd) => {
      commands.push(cmd);
      if (cmd === "search_jobs") {
        return search.promise;
      }
      if (cmd === "list_jobs") {
        return firstPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    store.setSearchInput("rust");
    store.search();
    store.refresh();

    search.fail({ code: "internal_error", messageId: "job.error.search" });

    await waitFor(() => expect(store.getSnapshot().jobs).toHaveLength(2));
    expect(store.getSnapshot().searchMessage).toBeNull();
    expect(store.getSnapshot().busy).toBeNull();
    expect(commands).toEqual(["search_jobs", "list_jobs"]);
  });
});
