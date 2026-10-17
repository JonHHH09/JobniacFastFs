import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { JobCaptureDto, JobDetailDto, JobPageDto } from "../../ipc/bindings";
import { JobsPage } from "./JobsPage";

// Synthetic identifiers, postings and links only. The link carries userinfo on purpose: the screen
// must never show it again.
const JOB_ID = "00000000-0000-4000-8000-0000000000c1";
const LINK = "https://synthetic-user:synthetic-secret@example.com/jobs/42";

const JOB: JobDetailDto = {
  job: {
    id: JOB_ID,
    revision: "1",
    title: "Synthetic Engineer",
    company: "Example Labs",
    location: "Remote",
    description: "Synthetic description of the role.",
    sourceLabel: "",
    employmentType: "",
    seniority: "",
    experienceRequirement: "",
    postedAt: "2026-01-02T03:04:05Z",
    sourceMethod: "link",
  },
  skills: ["Rust"],
};

const PAGE: JobPageDto = {
  jobs: [{ id: JOB_ID, title: "Synthetic Engineer", company: "Example Labs", location: "Remote" }],
  nextCursor: null,
};

const CAPTURE: JobCaptureDto = { status: "created_job", job: JOB };

type Overrides = Record<string, (payload: unknown) => unknown>;

/** A backend for the jobs page. Unknown commands fail, so a stray call is visible. */
function backend(composed: boolean, overrides: Overrides = {}) {
  const calls: Array<{ cmd: string; payload: unknown }> = [];
  mockIPC((cmd, payload) => {
    calls.push({ cmd, payload });
    const override = overrides[cmd];
    if (override) {
      return override(payload) as never;
    }
    if (cmd === "job_link_capabilities_get") {
      return { jobLink: composed } as never;
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

/** Waits until Rust has answered the capability, so the row is either shown or known to be hidden. */
async function waitForCapability(calls: Array<{ cmd: string }>) {
  await waitFor(() =>
    expect(calls.some(({ cmd }) => cmd === "job_link_capabilities_get")).toBe(true),
  );
}

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

/** Types into the link input once Rust has answered the capability and the row is shown. */
async function typeLink(value: string) {
  fireEvent.change(await screen.findByTestId("jobLinkUrlInput"), { target: { value } });
}

describe("add from link", () => {
  it("is hidden when the link capture is not composed", async () => {
    const calls = backend(false);
    renderPage();
    await waitForCapability(calls);
    await waitFor(() => expect(screen.queryByTestId("jobLinkRow")).toBeNull());
    expect(screen.getByTestId("jobPastePanel")).toBeInTheDocument();
  });

  it("is shown when composed, and its button needs a non-blank link", async () => {
    const calls = backend(true);
    renderPage();
    expect(await screen.findByTestId("jobLinkUrlInput")).toBeInTheDocument();
    await waitForCapability(calls);
    const button = screen.getByTestId("addJobFromLinkButton");
    expect(button).toBeDisabled();
    await typeLink("   ");
    expect(button).toBeDisabled();
    await typeLink(LINK);
    expect(button).toBeEnabled();
    expect(calls.some(({ cmd }) => cmd === "job_add_link")).toBe(false);
  });

  it("sends the trimmed link once, shows the progress copy and disables the controls", async () => {
    let finish: (value: JobCaptureDto) => void = () => {};
    const pending = new Promise<JobCaptureDto>((resolve) => {
      finish = resolve;
    });
    const calls = backend(true, { job_add_link: () => pending });
    renderPage();
    await typeLink(`  ${LINK}  `);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));

    expect(await screen.findByText("Fetching job link…")).toBeInTheDocument();
    expect(screen.getByTestId("jobLinkUrlInput")).toBeDisabled();
    expect(screen.getByTestId("addJobFromLinkButton")).toBeDisabled();
    expect(screen.getByTestId("addJobButton")).toBeDisabled();

    finish(CAPTURE);
    await screen.findByText("Job posting saved.");
    expect(calls.filter(({ cmd }) => cmd === "job_add_link").map(({ payload }) => payload)).toEqual(
      [{ url: LINK }],
    );
  });

  it("refreshes the saved-job list after a capture and selects no job", async () => {
    const calls = backend(true, { job_add_link: () => CAPTURE });
    renderPage();
    await typeLink(LINK);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));

    expect(await screen.findByText("Job posting saved.")).toBeInTheDocument();
    await waitFor(() => expect(calls.some(({ cmd }) => cmd === "list_jobs")).toBe(true));
    expect(calls.some(({ cmd }) => cmd === "get_job")).toBe(false);
    expect(screen.queryByTestId("jobEditorPanel")).toBeNull();
    // The link text leaves the input once the job is captured.
    expect((screen.getByTestId("jobLinkUrlInput") as HTMLInputElement).value).toBe("");
  });

  it("says the posting was already saved when Rust reuses the job for the same link", async () => {
    const calls = backend(true, {
      job_add_link: () => ({ ...CAPTURE, status: "reused_existing_job" }),
    });
    renderPage();
    await typeLink(LINK);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));

    expect(await screen.findByText("This posting was already saved.")).toBeInTheDocument();
    await waitFor(() => expect(calls.some(({ cmd }) => cmd === "list_jobs")).toBe(true));
    expect(calls.some(({ cmd }) => cmd === "get_job")).toBe(false);
  });

  it("maps a failure code to its copy and keeps the link in the input without echoing it", async () => {
    backend(true, {
      job_add_link: () => reject("validation_error", "job.linkError.redirect"),
    });
    renderPage();
    await typeLink(LINK);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));

    expect(
      await screen.findByText(
        "This link redirects, and redirects are not followed. Use the final link.",
      ),
    ).toBeInTheDocument();
    expect((screen.getByTestId("jobLinkUrlInput") as HTMLInputElement).value).toBe(LINK);
    expect(screen.queryByTestId("jobEditorPanel")).toBeNull();
    expect(document.body.textContent).not.toContain("synthetic-secret");
  });

  it("shows the content copy for a page without usable job content", async () => {
    backend(true, {
      job_add_link: () => reject("validation_error", "job.linkError.content"),
    });
    renderPage();
    await typeLink(LINK);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));
    expect(
      await screen.findByText(
        "The page did not return usable job content. Paste the job text instead.",
      ),
    ).toBeInTheDocument();
  });

  it("shows the generic copy for an unlisted failure, never the raw error text", async () => {
    backend(true, {
      job_add_link: () => reject("internal_error", "not.a.catalogue.id"),
    });
    renderPage();
    await typeLink(LINK);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));

    expect(
      await screen.findByText("Unable to add this job from the link. Please try again."),
    ).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("not.a.catalogue.id");
    expect(document.body.textContent).not.toContain("synthetic-secret");
  });

  it("keeps the saved-job list and editor unchanged after a failed capture", async () => {
    backend(true, {
      job_add_link: () => reject("validation_error", "job.linkError.unsafeAddress"),
    });
    renderPage();
    await typeLink(LINK);
    fireEvent.click(await screen.findByTestId("addJobFromLinkButton"));
    expect(
      await screen.findByText(
        "This link points to a private or unsafe address and was not fetched.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByTestId("jobSaveStatusLabel")).toBeEmptyDOMElement();
    expect(screen.queryByTestId("jobEditorPanel")).toBeNull();
  });

  it("does not send a blank link when Enter is pressed in the input", async () => {
    const calls = backend(true);
    renderPage();
    const input = await screen.findByTestId("jobLinkUrlInput");
    await typeLink("   ");
    fireEvent.submit(input.closest("form") as HTMLFormElement);
    await waitFor(() => expect(calls.some(({ cmd }) => cmd === "job_add_link")).toBe(false));
  });
});
