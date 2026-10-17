import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CLOSE_REFUSED_EVENT } from "../../app/closeRefusal";
import type {
  PersonalDetailsEnvelopeDto,
  ProfileGraphDto,
  ProfileSearchDto,
} from "../../ipc/bindings";
import { ProfilesPage } from "./ProfilesPage";

type Listener = (event: { payload: unknown }) => void;

// The listeners the close hooks registered, by event name. Hoisted so the mock factory can use it.
const listeners = vi.hoisted(() => new Map<string, Listener>());

vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, listener: Listener) => {
    listeners.set(name, listener);
    return Promise.resolve(() => {
      listeners.delete(name);
    });
  },
}));

// Synthetic identifiers and personal data only.
const ADA_ID = "00000000-0000-4000-8000-0000000000a1";
const EXPERIENCE_ID = "00000000-0000-4000-8000-0000000000e1";

function graphFor(summary: string, revision: string): ProfileGraphDto {
  return {
    profile: {
      id: ADA_ID,
      fullName: "Ada Example",
      email: "ada@example.test",
      summary,
      revision,
      createdAt: "2026-01-02T03:04:05Z",
      updatedAt: "2026-01-02T03:04:05Z",
    },
    contacts: [],
    links: [],
    skills: [],
    languages: [],
    education: [],
    experiences: [
      {
        id: EXPERIENCE_ID,
        company: "Example Labs",
        title: "Engineer",
        location: null,
        startDate: "2020-01-01",
        endDate: null,
        description: null,
        displayOrder: 0,
      },
    ],
    projects: [],
  };
}

const adaPersonal: PersonalDetailsEnvelopeDto = {
  profileId: ADA_ID,
  profileRevision: "7",
  profileUpdatedAt: "2026-01-02T03:04:05Z",
  personalDetails: null,
};

const adaSearch: ProfileSearchDto = {
  query: "ada",
  queryTokens: ["ada"],
  totalMatches: "1",
  matchedCount: "1",
  hasMore: false,
  returnedCount: 1,
  profiles: [
    {
      profile: { id: ADA_ID, fullName: "Ada Example", email: "ada@example.test" },
      matchedFields: ["fullName", "email"],
      score: "10",
    },
  ],
};

/** The commands the page sent, in order, with their payloads. */
type Call = { cmd: string; payload: unknown };

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ProfilesPage />
    </QueryClientProvider>,
  );
}

/** Serves the Ada profile; `update_profile` answers with the saved graph. */
function mockBackend(calls: Call[]) {
  mockIPC((cmd, payload) => {
    calls.push({ cmd, payload });
    if (cmd === "search_profiles") {
      return adaSearch;
    }
    if (cmd === "get_profile") {
      return graphFor("Analyst", "7");
    }
    if (cmd === "get_personal_details") {
      return adaPersonal;
    }
    if (cmd === "update_profile") {
      return graphFor("Updated summary", "8");
    }
    if (cmd === "shell_set_dirty" || cmd === "shell_request_close") {
      return undefined;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
}

/** The dirty reports sent to Rust, in order. */
function dirtyReports(calls: Call[]): Array<{ workspace: string; dirty: boolean }> {
  return calls
    .filter((call) => call.cmd === "shell_set_dirty")
    .map((call) => call.payload as { workspace: string; dirty: boolean });
}

/** Runs a search and opens its only result, then waits for the loaded profile. */
async function openAda() {
  fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
  fireEvent.click(screen.getByTestId("searchProfilesButton"));
  fireEvent.click(await screen.findByRole("button", { name: "Open profile Ada Example" }));
  expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
}

function editSummary(value: string) {
  fireEvent.change(within(screen.getByTestId("profileEditorPanel")).getByTestId("summaryInput"), {
    target: { value },
  });
}

/** Lets the close hooks register their Rust event listeners. */
async function settleListeners() {
  await act(async () => {
    await Promise.resolve();
  });
}

/** The Rust gate refused a close because the profile draft is unsaved. */
function refuseCloseForUnsavedProfile() {
  act(() => {
    listeners.get(CLOSE_REFUSED_EVENT)?.({
      payload: {
        workspace: "profile",
        reason: "confirm_discard",
        messageId: "profile.confirm.close",
      },
    });
  });
}

describe("profile editor dirty reports for the window close gate", () => {
  afterEach(() => {
    clearMocks();
    listeners.clear();
  });

  it("reports the draft dirty on an edit and clean again after the save", async () => {
    const calls: Call[] = [];
    mockBackend(calls);

    renderPage();
    await settleListeners();
    await openAda();
    expect(dirtyReports(calls)).toEqual([]);

    editSummary("Updated summary");
    await waitFor(() =>
      expect(dirtyReports(calls)).toEqual([{ workspace: "profile", dirty: true }]),
    );

    fireEvent.click(screen.getByTestId("saveProfileButton"));
    expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
    await waitFor(() =>
      expect(dirtyReports(calls)).toEqual([
        { workspace: "profile", dirty: true },
        { workspace: "profile", dirty: false },
      ]),
    );
  });

  it("keeps the window and the edit when the discard confirmation is cancelled", async () => {
    const calls: Call[] = [];
    mockBackend(calls);

    renderPage();
    await settleListeners();
    await openAda();
    editSummary("Unsaved words");
    await waitFor(() => expect(dirtyReports(calls)).toHaveLength(1));

    refuseCloseForUnsavedProfile();
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Discard unsaved changes?")).toBeInTheDocument();
    expect(
      within(dialog).getByText("Discard your unsaved edits and close Jobniac?"),
    ).toBeInTheDocument();

    fireEvent.click(within(dialog).getByTestId("cancelUnsavedActionButton"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());

    expect(
      within(screen.getByTestId("profileEditorPanel")).getByTestId("summaryInput"),
    ).toHaveValue("Unsaved words");
    expect(calls.some((call) => call.cmd === "shell_request_close")).toBe(false);
    expect(dirtyReports(calls)).toEqual([{ workspace: "profile", dirty: true }]);
  });

  it("reports the draft clean and then asks Rust to close when the discard is confirmed", async () => {
    const calls: Call[] = [];
    mockBackend(calls);

    renderPage();
    await settleListeners();
    await openAda();
    editSummary("Unsaved words");
    await waitFor(() => expect(dirtyReports(calls)).toHaveLength(1));

    refuseCloseForUnsavedProfile();
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByTestId("confirmUnsavedActionButton"));

    await waitFor(() =>
      expect(calls.some((call) => call.cmd === "shell_request_close")).toBe(true),
    );
    const cleanIndex = calls.findIndex(
      (call) =>
        call.cmd === "shell_set_dirty" && (call.payload as { dirty: boolean }).dirty === false,
    );
    const closeIndex = calls.findIndex((call) => call.cmd === "shell_request_close");
    expect(cleanIndex).toBeGreaterThanOrEqual(0);
    expect(cleanIndex).toBeLessThan(closeIndex);
    expect(
      within(screen.getByTestId("profileEditorPanel")).getByTestId("summaryInput"),
    ).not.toHaveValue("Unsaved words");
  });

  it("reports clean when the page unmounts with unsaved edits", async () => {
    const calls: Call[] = [];
    mockBackend(calls);

    const { unmount } = renderPage();
    await settleListeners();
    await openAda();
    editSummary("Unsaved words");
    await waitFor(() => expect(dirtyReports(calls)).toHaveLength(1));

    unmount();
    const reports = dirtyReports(calls);
    expect(reports[reports.length - 1]).toEqual({ workspace: "profile", dirty: false });
  });
});
