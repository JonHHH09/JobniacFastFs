import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { requestOpen } from "../../app/openRequests";
import type {
  PersonalDetailsEnvelopeDto,
  ProfileGraphDto,
  ProfilePageDto,
  ProfileSearchDto,
} from "../../ipc/bindings";
import { ProfilesPage } from "./ProfilesPage";

// Synthetic identifiers and personal data only.
const ADA_ID = "00000000-0000-4000-8000-0000000000a1";
const GRACE_ID = "00000000-0000-4000-8000-0000000000b2";
const EXPERIENCE_ID = "00000000-0000-4000-8000-0000000000e1";

const firstPage: ProfilePageDto = {
  profiles: [{ id: ADA_ID, fullName: "Ada Example", email: "ada@example.test" }],
  nextCursor: "cursor-2",
};

const secondPage: ProfilePageDto = {
  profiles: [{ id: GRACE_ID, fullName: "Grace Sample", email: "grace@example.test" }],
  nextCursor: null,
};

function graphFor(id: string, fullName: string, summary: string, revision = "7"): ProfileGraphDto {
  return {
    profile: {
      id,
      fullName,
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

const adaGraph = graphFor(ADA_ID, "Ada Example", "Analyst");

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

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ProfilesPage />
    </QueryClientProvider>,
  );
}

/**
 * The commands a mocked invoke received, without the event-bridge registrations (the close gate
 * listens to its refusal event on mount).
 */
function commandsOf(invoke: { mock: { calls: unknown[][] } }): string[] {
  return invoke.mock.calls
    .map(([cmd]) => cmd as string)
    .filter((cmd) => cmd !== "plugin:event|listen");
}

/** A command that fails the way the Rust side rejects: a typed `{ code, messageId }` object. */
function reject(code: string, messageId: string): never {
  throw { code, messageId };
}

function listSection() {
  return screen.getByTestId("profileListPanel");
}

function editorSection() {
  return screen.getByTestId("profileEditorPanel");
}

describe("profile list", () => {
  it("does not load at startup and shows the idle copy", () => {
    const invoke = vi.fn(() => firstPage);
    mockIPC(invoke);

    renderPage();

    expect(screen.getByTestId("profileStatusLabel")).toHaveTextContent("Refresh to load profiles.");
    // The close-gate listener registers with the event bridge on mount; no command is sent.
    expect(commandsOf(invoke)).toEqual([]);
  });

  it("loads page one on refresh and appends the next page", async () => {
    const seen: unknown[] = [];
    mockIPC((cmd, payload) => {
      if (cmd === "list_profiles") {
        seen.push(payload);
        return (payload as { cursor: string | null }).cursor === null ? firstPage : secondPage;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    expect(await screen.findByText("Ada Example")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("nextProfilePageButton"));
    expect(await screen.findByText("Grace Sample")).toBeInTheDocument();
    expect(screen.getByText("Ada Example")).toBeInTheDocument();
    expect(seen).toEqual([{ cursor: null }, { cursor: "cursor-2" }]);
    expect(screen.queryByTestId("profileStatusLabel")).not.toBeInTheDocument();
  });

  it("opens a saved profile when its row is clicked", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_profiles") {
        return firstPage;
      }
      if (cmd === "get_profile") {
        return adaGraph;
      }
      if (cmd === "get_personal_details") {
        return adaPersonal;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    fireEvent.click(await within(listSection()).findByRole("button", { name: /Ada Example/ }));

    expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
  });

  it("opens the profile a command palette request names", async () => {
    mockIPC((cmd) => {
      if (cmd === "get_profile") {
        return adaGraph;
      }
      if (cmd === "get_personal_details") {
        return adaPersonal;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    act(() => requestOpen({ kind: "profile", id: ADA_ID }));

    expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
  });

  it("keeps the rows already shown when a refresh fails", async () => {
    let calls = 0;
    mockIPC((cmd) => {
      if (cmd === "list_profiles") {
        calls += 1;
        return calls === 1 ? firstPage : reject("internal_error", "profiles.error");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    await screen.findByText("Ada Example");

    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    expect(
      await screen.findByText("Unable to load profiles. Please try again."),
    ).toBeInTheDocument();
    expect(screen.getByText("Ada Example")).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("internal_error");
  });

  it("shows the not-configured copy when the cursor key is missing", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_profiles") {
        return reject("not_configured", "profiles.unconfigured");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));

    expect(await screen.findByText("Profile service is not configured.")).toBeInTheDocument();
    expect(screen.getByTestId("profileRefreshButton")).toBeDisabled();
  });
});

describe("find a profile", () => {
  it("sends no request for a blank query and says what to enter", async () => {
    const invoke = vi.fn(() => adaSearch);
    mockIPC(invoke);

    renderPage();
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "   " } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));

    expect(
      await screen.findByText("Enter a name, email, skill, or role to search."),
    ).toBeInTheDocument();
    expect(commandsOf(invoke)).toEqual([]);
  });

  it("shows the results, the matched fields and the summary", async () => {
    mockIPC((cmd, payload) => {
      if (cmd === "search_profiles") {
        expect(payload).toEqual({ query: "ada" });
        return adaSearch;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: " ada " } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));

    expect(
      await screen.findByText("Showing 1 results; 1 matched and 1 total."),
    ).toBeInTheDocument();
    expect(screen.getByText("Matched fields: Name, Email")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open profile Ada Example" })).toBeInTheDocument();
  });

  it("shows the failure copy whatever the code", async () => {
    mockIPC((cmd) => {
      if (cmd === "search_profiles") {
        return reject("upstream_unavailable", "profileSearch.failed");
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));

    expect(
      await screen.findByText("Unable to search profiles. Please try again."),
    ).toBeInTheDocument();
  });
});

describe("profile editor", () => {
  /** Runs a search and opens its only result, then waits for the loaded profile. */
  async function openAda() {
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));
    fireEvent.click(await screen.findByRole("button", { name: "Open profile Ada Example" }));
    expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
  }

  function commonMock(overrides: (cmd: string, payload: unknown) => unknown = () => undefined) {
    mockIPC((cmd, payload) => {
      const override = overrides(cmd, payload);
      if (override !== undefined) {
        return override as never;
      }
      if (cmd === "search_profiles") {
        return adaSearch;
      }
      if (cmd === "get_profile") {
        return adaGraph;
      }
      if (cmd === "get_personal_details") {
        return adaPersonal;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
  }

  it("loads an opened result and saves an edit with the loaded revision", async () => {
    const updated = graphFor(ADA_ID, "Ada Example", "Updated summary", "8");
    const update = vi.fn(() => updated);
    commonMock((cmd) => (cmd === "update_profile" ? update() : undefined));

    renderPage();
    await openAda();
    expect(within(editorSection()).getByTestId("fullNameInput")).toHaveValue("Ada Example");
    expect(screen.getByTestId("saveProfileButton")).toBeDisabled();

    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Updated summary" },
    });
    expect(screen.getByTestId("saveProfileButton")).toBeEnabled();
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
    expect(update).toHaveBeenCalledTimes(1);
  });

  it("sends the revision as text and the write keys as request text", async () => {
    const seen: Array<{ profileId: string; expectedRevision: string; requestJson: string }> = [];
    commonMock((cmd, payload) => {
      if (cmd === "update_profile") {
        seen.push(payload as (typeof seen)[number]);
        return graphFor(ADA_ID, "Ada Example", "Updated summary", "8");
      }
      return undefined;
    });

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Updated summary" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));
    await screen.findByText("Profile saved.");

    expect(seen).toHaveLength(1);
    expect(seen[0]?.profileId).toBe(ADA_ID);
    expect(seen[0]?.expectedRevision).toBe("7");
    const request = JSON.parse(seen[0]?.requestJson ?? "{}") as Record<string, unknown>;
    expect(request.summary).toBe("Updated summary");
    expect(request.experiences).toEqual([
      expect.objectContaining({ id: EXPERIENCE_ID, company: "Example Labs", endDate: null }),
    ]);
    // Every collection of a loaded graph is provided, so an empty one is still sent as [].
    expect(request.contacts).toEqual([]);
  });

  it("shows the conflict copy and keeps the draft when the revision is stale", async () => {
    commonMock((cmd) =>
      cmd === "update_profile" ? reject("conflict", "profile.error.conflict") : undefined,
    );

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Edited elsewhere" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(
      await screen.findByText(
        "This profile changed since it was loaded. Your draft is unchanged; reload the latest version before saving again.",
      ),
    ).toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Edited elsewhere");
  });

  it("asks before replacing an unsaved draft and keeps the edits when asked to", async () => {
    commonMock();

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });
    fireEvent.click(screen.getByTestId("newProfileButton"));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Discard unsaved changes?")).toBeInTheDocument();
    expect(
      within(dialog).getByText("Discard your unsaved edits and start a new profile?"),
    ).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Keep the current profile draft" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    });
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved words");
  });

  const graceGraph = graphFor(GRACE_ID, "Grace Sample", "Grace summary");

  function listMock() {
    commonMock((cmd, payload) => {
      if (cmd === "list_profiles") {
        return { profiles: [...firstPage.profiles, ...secondPage.profiles], nextCursor: null };
      }
      if (cmd === "get_profile") {
        return JSON.stringify(payload).includes(GRACE_ID) ? graceGraph : adaGraph;
      }
      if (cmd === "get_personal_details") {
        return JSON.stringify(payload).includes(GRACE_ID)
          ? { ...adaPersonal, profileId: GRACE_ID }
          : adaPersonal;
      }
      return undefined;
    });
  }

  async function dirtyAdaWithList() {
    listMock();
    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    await within(listSection()).findByRole("button", { name: /Grace Sample/ });
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });
  }

  it("asks before a row click replaces an unsaved draft, and Discard loads the row", async () => {
    await dirtyAdaWithList();
    fireEvent.click(within(listSection()).getByRole("button", { name: /Grace Sample/ }));

    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard the draft and continue" }));
    await waitFor(() => {
      expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Grace summary");
    });
  });

  it("asks before a palette request replaces an unsaved draft", async () => {
    await dirtyAdaWithList();
    act(() => requestOpen({ kind: "profile", id: GRACE_ID }));

    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard the draft and continue" }));
    await waitFor(() => {
      expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Grace summary");
    });
  });

  it("keeps a row a button while busy and ignores palette requests during a load", async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    mockIPC(async (cmd) => {
      if (cmd === "list_profiles") {
        return firstPage;
      }
      if (cmd === "get_profile") {
        await gate;
        return adaGraph;
      }
      if (cmd === "get_personal_details") {
        return adaPersonal;
      }
      throw new Error(`unexpected command ${cmd}`);
    });
    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    const row = await within(listSection()).findByRole("button", { name: /Ada Example/ });
    row.focus();
    fireEvent.click(row);

    expect(within(listSection()).getByRole("button", { name: /Ada Example/ })).toBe(row);
    // A palette request during the load neither opens the dialog nor starts a second load.
    act(() => requestOpen({ kind: "profile", id: GRACE_ID }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

    release?.();
    expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
    expect(within(listSection()).getByRole("button", { name: /Ada Example/ })).toBe(row);
  });

  it("starts a blank profile after the discard is confirmed", async () => {
    commonMock();

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });
    fireEvent.click(screen.getByTestId("newProfileButton"));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard the draft and continue" }));

    expect(
      await screen.findByText("Enter profile details, then save the new profile."),
    ).toBeInTheDocument();
    expect(within(editorSection()).getByTestId("fullNameInput")).toHaveValue("");
    expect(screen.getByTestId("saveProfileButton")).toBeEnabled();
  });

  it("shows no form fields before a profile is opened", () => {
    commonMock();
    renderPage();
    expect(screen.queryByTestId("fullNameInput")).not.toBeInTheDocument();
    expect(screen.queryByTestId("personalDetailsSection")).not.toBeInTheDocument();
    expect(screen.getByTestId("saveProfileButton")).toBeDisabled();
    expect(listSection()).toBeInTheDocument();
  });

  it("shows no 'not provided' hint and does not ask to discard an untouched new profile", () => {
    commonMock();
    renderPage();
    fireEvent.click(screen.getByTestId("newProfileButton"));
    expect(within(editorSection()).getByTestId("fullNameInput")).toHaveValue("");
    expect(
      screen.queryByText("This collection was not provided. Adding an entry will include it."),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("newProfileButton"));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("does not reload the loaded profile when it is selected again while clean", async () => {
    const getProfile = vi.fn(() => adaGraph);
    commonMock((cmd) => (cmd === "get_profile" ? getProfile() : undefined));

    renderPage();
    await openAda();
    expect(getProfile).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Open profile Ada Example" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(getProfile).toHaveBeenCalledTimes(1);
  });

  it("refuses a saved response whose revision is not the loaded revision plus one", async () => {
    const wrongRevision = {
      ...graphFor(ADA_ID, "Ada Example", "Updated summary"),
      profile: { ...graphFor(ADA_ID, "Ada Example", "x").profile, revision: "9" },
    };
    commonMock((cmd) => (cmd === "update_profile" ? wrongRevision : undefined));

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Updated summary" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(
      await screen.findByText("Unable to save this profile. Please try again."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Profile saved.")).not.toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Updated summary");
  });

  it("accepts a saved revision above 2^53 - 1 when it is the loaded revision plus one", async () => {
    // Revisions are i64 on the Rust side: the accept check must not round them through Number.
    const loaded = "9007199254740993";
    const next = "9007199254740994";
    editorBackend(
      graphWith({}, loaded),
      {
        update_profile: () => graphFor(ADA_ID, "Ada Example", "Large revision", next),
      },
      { ...adaPersonal, profileRevision: loaded },
    );

    renderPage();
    await openEditorProfile();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Large revision" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
    expect(
      screen.queryByText("Unable to save this profile. Please try again."),
    ).not.toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Large revision");
  });

  it("shows no save error when Rust refuses the save because another operation runs", async () => {
    commonMock((cmd) => (cmd === "update_profile" ? reject("busy", "profile.busy") : undefined));

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Edited" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    await waitFor(() => expect(screen.getByTestId("saveProfileButton")).toBeEnabled());
    expect(
      screen.queryByText("Unable to save this profile. Please try again."),
    ).not.toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Edited");
  });

  it("keeps the draft when Escape cancels the discard confirmation", async () => {
    commonMock();

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });
    fireEvent.click(screen.getByTestId("newProfileButton"));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.keyDown(dialog, { key: "Escape" });

    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved words");
  });

  it("saves personal details without touching the profile draft", async () => {
    const seen: unknown[] = [];
    const personalSaved: PersonalDetailsEnvelopeDto = {
      profileId: ADA_ID,
      profileRevision: "8",
      profileUpdatedAt: "2026-01-02T03:04:05Z",
      personalDetails: {
        profileId: ADA_ID,
        dateOfBirth: "1990-01-02",
        nationality: "Synthland",
        photoDocumentId: null,
        createdAt: "2026-01-02T03:04:05Z",
        updatedAt: "2026-01-02T03:04:05Z",
      },
    };
    commonMock((cmd, payload) => {
      if (cmd === "update_personal_details") {
        seen.push(payload);
        return personalSaved;
      }
      return undefined;
    });

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });
    fireEvent.change(within(editorSection()).getByTestId("nationalityInput"), {
      target: { value: "Synthland" },
    });
    fireEvent.click(screen.getByTestId("savePersonalDetailsButton"));

    expect(await screen.findByText("Personal details saved.")).toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved words");
    expect(seen).toEqual([expect.objectContaining({ profileId: ADA_ID, expectedRevision: "7" })]);
  });

  it("keeps both drafts when a personal save fails", async () => {
    commonMock((cmd) =>
      cmd === "update_personal_details" ? reject("conflict", "profile.error.conflict") : undefined,
    );

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });
    fireEvent.change(within(editorSection()).getByTestId("nationalityInput"), {
      target: { value: "Synthland" },
    });
    fireEvent.click(screen.getByTestId("savePersonalDetailsButton"));

    expect(
      await screen.findByText(
        "This profile changed since it was loaded. Your draft is unchanged; reload the latest version before saving again.",
      ),
    ).toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved words");
    expect(within(editorSection()).getByTestId("nationalityInput")).toHaveValue("Synthland");
  });

  it("hides the 'not configured' personal hint while a save is pending", async () => {
    commonMock((cmd) => (cmd === "update_profile" ? new Promise(() => undefined) : undefined));

    renderPage();
    await openAda();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Edited" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    await waitFor(() => expect(screen.getByTestId("saveProfileButton")).toBeDisabled());
    expect(screen.queryByText("Personal details are not configured.")).not.toBeInTheDocument();
  });
});

describe("profile list and search states", () => {
  it("keeps Refresh disabled while the next page loads", async () => {
    mockIPC((cmd, payload) => {
      if (cmd === "list_profiles") {
        return (payload as { cursor: string | null }).cursor === null
          ? firstPage
          : new Promise(() => undefined);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    await screen.findByText("Ada Example");
    fireEvent.click(screen.getByTestId("nextProfilePageButton"));

    await waitFor(() => expect(screen.getByTestId("profileRefreshButton")).toBeDisabled());
    expect(screen.getByTestId("profileStatusLabel")).toHaveTextContent("Loading profiles…");
  });

  it("shows the loading label, not the failure, while a next page loads after a failed refresh", async () => {
    let calls = 0;
    mockIPC((cmd) => {
      if (cmd === "list_profiles") {
        calls += 1;
        if (calls === 1) {
          return firstPage;
        }
        if (calls === 2) {
          return reject("internal_error", "profiles.error");
        }
        return new Promise(() => undefined);
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    await screen.findByText("Ada Example");
    fireEvent.click(screen.getByTestId("profileRefreshButton"));
    await screen.findByText("Unable to load profiles. Please try again.");

    fireEvent.click(screen.getByTestId("nextProfilePageButton"));
    await waitFor(() =>
      expect(screen.getByTestId("profileStatusLabel")).toHaveTextContent("Loading profiles…"),
    );
  });

  it("runs the same search again after it failed", async () => {
    let calls = 0;
    mockIPC((cmd) => {
      if (cmd === "search_profiles") {
        calls += 1;
        return calls === 1 ? reject("upstream_unavailable", "profileSearch.failed") : adaSearch;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));
    await screen.findByText("Unable to search profiles. Please try again.");

    fireEvent.click(screen.getByTestId("searchProfilesButton"));
    expect(
      await screen.findByText("Showing 1 results; 1 matched and 1 total."),
    ).toBeInTheDocument();
    expect(calls).toBe(2);
  });

  it("disables Save and edits while a search runs", async () => {
    let hang = false;
    mockIPC((cmd, payload) => {
      if (cmd === "search_profiles" && hang) {
        return new Promise(() => undefined);
      }
      if (cmd === "search_profiles") {
        return adaSearch;
      }
      if (cmd === "get_profile") {
        return adaGraph;
      }
      if (cmd === "get_personal_details") {
        return adaPersonal;
      }
      throw new Error(`unexpected command ${cmd}: ${JSON.stringify(payload)}`);
    });

    renderPage();
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));
    fireEvent.click(await screen.findByRole("button", { name: "Open profile Ada Example" }));
    await screen.findByText("Profile loaded.");
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Edited" },
    });
    expect(screen.getByTestId("saveProfileButton")).toBeEnabled();

    hang = true;
    fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "grace" } });
    fireEvent.click(screen.getByTestId("searchProfilesButton"));

    await waitFor(() => expect(screen.getByTestId("saveProfileButton")).toBeDisabled());
    expect(within(editorSection()).getByTestId("summaryInput")).toBeDisabled();
  });
});

// Ids and stamps of the editor rules below: synthetic values only.
const SKILL_ID = "00000000-0000-4000-8000-0000000000f1";
const PROJECT_ID = "00000000-0000-4000-8000-0000000000c1";
const OTHER_PROJECT_ID = "00000000-0000-4000-8000-0000000000c2";
const TECHNOLOGY_ID = "00000000-0000-4000-8000-0000000000d1";
const PERSONAL_STAMP = "2026-01-02T03:04:05Z";

/** Ada's graph with the given collections replaced (every other collection keeps its fixture). */
function graphWith(collections: Record<string, unknown[]>, revision = "7"): ProfileGraphDto {
  return {
    ...graphFor(ADA_ID, "Ada Example", "Analyst", revision),
    ...collections,
  } as unknown as ProfileGraphDto;
}

/** The graph a successful save returns: the same content at the given revision. */
function savedGraph(graph: ProfileGraphDto, revision: string): ProfileGraphDto {
  return { ...graph, profile: { ...graph.profile, revision } };
}

/** Decodes the request text a write command received. */
function requestOf(payload: unknown): Record<string, unknown> {
  return JSON.parse((payload as { requestJson: string }).requestJson) as Record<string, unknown>;
}

/** The mocked backend of an editor test: the overrides first, then the Ada fixtures. */
function editorBackend(
  graph: ProfileGraphDto,
  overrides: Record<string, (payload: unknown) => unknown> = {},
  personal: PersonalDetailsEnvelopeDto = adaPersonal,
) {
  mockIPC((cmd, payload) => {
    const override = overrides[cmd];
    if (override) {
      return override(payload) as never;
    }
    if (cmd === "search_profiles") {
      return adaSearch as never;
    }
    if (cmd === "get_profile") {
      return graph as never;
    }
    if (cmd === "get_personal_details") {
      return personal as never;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
}

/** Opens Ada from her search result and waits until the editor shows the loaded profile. */
async function openEditorProfile() {
  fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
  fireEvent.click(screen.getByTestId("searchProfilesButton"));
  fireEvent.click(await screen.findByRole("button", { name: "Open profile Ada Example" }));
  expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
}

describe("profile editor display orders and normalized fields", () => {
  it("UI-PRF-006 a skill entry added in the form is sent with its name and the next order", async () => {
    const requests: Array<Record<string, unknown>> = [];
    editorBackend(graphWith({ skills: [] }), {
      update_profile: (payload) => {
        requests.push(requestOf(payload));
        return savedGraph(graphWith({ skills: [] }), "8");
      },
    });

    renderPage();
    await openEditorProfile();
    fireEvent.click(within(editorSection()).getByTestId("addSkillsButton"));
    fireEvent.change(within(editorSection()).getByTestId("skills_0_skillInput"), {
      target: { value: "Python" },
    });
    fireEvent.change(within(editorSection()).getByTestId("skills_0_categoryInput"), {
      target: { value: "Programming" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
    // The new row has no id; its normalized key is null, as the presenter writes it, and Rust
    // derives the normalized skill on save.
    expect(requests[0]?.skills).toEqual([
      { skill: "Python", normalizedSkill: null, category: "Programming", displayOrder: 0 },
    ]);
  });

  it("UI-PRF-006 renaming a skill sends its normalized key as null on save", async () => {
    const skills = [
      {
        id: SKILL_ID,
        skill: "Python",
        normalizedSkill: "python",
        category: null,
        displayOrder: 0,
      },
    ];
    const requests: Array<Record<string, unknown>> = [];
    editorBackend(graphWith({ skills }), {
      update_profile: (payload) => {
        requests.push(requestOf(payload));
        return savedGraph(graphWith({ skills }), "8");
      },
    });

    renderPage();
    await openEditorProfile();
    fireEvent.change(within(editorSection()).getByTestId("skills_0_skillInput"), {
      target: { value: "Rust" },
    });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
    const savedSkills = requests[0]?.skills as Array<Record<string, unknown>>;
    const skill = savedSkills[0];
    expect(skill).toEqual({
      id: SKILL_ID,
      skill: "Rust",
      normalizedSkill: null,
      category: null,
      displayOrder: 0,
    });
  });

  it.each([
    {
      key: "skills",
      add: "addSkillsButton",
      field: "skill",
      existing: "Python",
      child: {
        id: SKILL_ID,
        skill: "Python",
        normalizedSkill: "python",
        category: null,
        displayOrder: 7,
      },
      value: "Rust",
    },
    {
      key: "languages",
      add: "addLanguagesButton",
      field: "language",
      existing: "English",
      child: {
        id: SKILL_ID,
        language: "English",
        normalizedLanguage: "english",
        proficiency: null,
        displayOrder: 7,
      },
      value: "German",
    },
    {
      key: "experiences",
      add: "addExperiencesButton",
      field: "company",
      existing: "Example Labs",
      child: {
        id: EXPERIENCE_ID,
        company: "Example Labs",
        title: "Engineer",
        location: null,
        startDate: "2020-01-01",
        endDate: null,
        description: null,
        displayOrder: 7,
      },
      value: "Sample Works",
    },
    {
      key: "projects",
      add: "addProjectsButton",
      field: "name",
      existing: "First",
      child: {
        id: PROJECT_ID,
        name: "First",
        url: null,
        description: null,
        displayOrder: 7,
        technologies: [],
      },
      value: "Second",
    },
  ])(
    "UI-PRF-012 an entry appended to $key stays after the sparse display order 7",
    async ({ key, add, field, existing, child, value }) => {
      const requests: Array<Record<string, unknown>> = [];
      editorBackend(graphWith({ [key]: [child] }), {
        update_profile: (payload) => {
          requests.push(requestOf(payload));
          return savedGraph(graphWith({ [key]: [child] }), "8");
        },
      });

      renderPage();
      await openEditorProfile();
      fireEvent.click(within(editorSection()).getByTestId(add));
      expect(within(editorSection()).getByTestId(`${key}_0_${field}Input`)).toHaveValue(existing);
      const appended = within(editorSection()).getByTestId(`${key}_1_${field}Input`);
      expect(appended).toHaveValue("");
      fireEvent.change(appended, { target: { value } });
      fireEvent.click(screen.getByTestId("saveProfileButton"));

      expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
      const rows = requests[0]?.[key] as Array<Record<string, unknown>>;
      expect(rows.map((row) => row.displayOrder)).toEqual([7, 8]);
      expect(rows[0]?.id).toBe(child.id);
      expect(rows[1]?.[field]).toBe(value);
    },
  );

  it("UI-PRF-013 a technology appended to a project stays after the sparse display order 7", async () => {
    const projects = [
      {
        id: PROJECT_ID,
        name: "First",
        url: null,
        description: null,
        displayOrder: 7,
        technologies: [
          {
            id: TECHNOLOGY_ID,
            technology: "Python",
            normalizedTechnology: "python",
            displayOrder: 7,
          },
        ],
      },
    ];
    const requests: Array<Record<string, unknown>> = [];
    editorBackend(graphWith({ projects }), {
      update_profile: (payload) => {
        requests.push(requestOf(payload));
        return savedGraph(graphWith({ projects }), "8");
      },
    });

    renderPage();
    await openEditorProfile();
    fireEvent.click(within(editorSection()).getByTestId("addProjectTechnology0Button"));
    const appended = within(editorSection()).getByTestId("projects_0_technology_1_input");
    expect(appended).toHaveValue("");
    fireEvent.change(appended, { target: { value: "Rust" } });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    expect(await screen.findByText("Profile saved.")).toBeInTheDocument();
    const savedProjects = requests[0]?.projects as Array<{
      technologies: Array<Record<string, unknown>>;
    }>;
    const technologies = savedProjects[0]?.technologies ?? [];
    expect(technologies.map((technology) => technology.displayOrder)).toEqual([7, 8]);
    expect(technologies[0]?.id).toBe(TECHNOLOGY_ID);
    expect(technologies[1]?.technology).toBe("Rust");
  });
});

describe("project-only save", () => {
  it("UI-PRF-009 keeps the other unsaved draft edits and sends the current revision", async () => {
    const projects = [
      {
        id: PROJECT_ID,
        name: "First",
        url: null,
        description: null,
        displayOrder: 7,
        technologies: [],
      },
      {
        id: OTHER_PROJECT_ID,
        name: "Second",
        url: null,
        description: null,
        displayOrder: 8,
        technologies: [],
      },
    ];
    const requests: Array<Record<string, unknown>> = [];
    editorBackend(
      graphWith({ projects }, "12"),
      {
        update_project: (payload) => {
          const sent = requestOf(payload);
          requests.push(sent);
          // Echo the saved name so the form keeps the value the user typed.
          return {
            project: { ...projects[0], name: sent.name as string },
            profileRevision: String(12 + requests.length),
          };
        },
      },
      { ...adaPersonal, profileRevision: "12" },
    );

    renderPage();
    await openEditorProfile();
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved summary" },
    });
    fireEvent.change(within(editorSection()).getByTestId("projects_1_nameInput"), {
      target: { value: "Second unsaved" },
    });
    fireEvent.change(within(editorSection()).getByTestId("projects_0_nameInput"), {
      target: { value: "Renamed" },
    });
    fireEvent.click(within(editorSection()).getByTestId("saveProjectFields0Button"));

    expect(
      await screen.findByText("Project fields saved. Other profile edits remain in your draft."),
    ).toBeInTheDocument();
    expect(requests[0]).toEqual({
      profileId: ADA_ID,
      projectId: PROJECT_ID,
      expectedRevision: "12",
      name: "Renamed",
    });
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved summary");
    expect(within(editorSection()).getByTestId("projects_1_nameInput")).toHaveValue(
      "Second unsaved",
    );
    expect(screen.getByTestId("saveProfileButton")).toBeEnabled();

    // The next project save sends the revision the first one returned (13), not the loaded 12.
    fireEvent.change(within(editorSection()).getByTestId("projects_0_nameInput"), {
      target: { value: "Renamed again" },
    });
    fireEvent.click(within(editorSection()).getByTestId("saveProjectFields0Button"));
    await waitFor(() => expect(requests).toHaveLength(2));
    expect(requests[1]).toEqual({
      profileId: ADA_ID,
      projectId: PROJECT_ID,
      expectedRevision: "13",
      name: "Renamed again",
    });
    expect(within(editorSection()).getByTestId("projects_1_nameInput")).toHaveValue(
      "Second unsaved",
    );
  });
});

describe("personal details save response", () => {
  const details = {
    profileId: ADA_ID,
    dateOfBirth: "1992-02-29",
    nationality: "Albanian",
    photoDocumentId: null,
    createdAt: PERSONAL_STAMP,
    updatedAt: PERSONAL_STAMP,
  };
  const validSaved: PersonalDetailsEnvelopeDto = {
    profileId: ADA_ID,
    profileRevision: "8",
    profileUpdatedAt: PERSONAL_STAMP,
    personalDetails: details,
  };

  it.each([
    { label: "another profile", mutation: { profileId: GRACE_ID } },
    { label: "the loaded revision", mutation: { profileRevision: "7" } },
    { label: "a skipped revision", mutation: { profileRevision: "9" } },
    { label: "a revision that is not a number", mutation: { profileRevision: "eight" } },
    { label: "an invalid timestamp", mutation: { profileUpdatedAt: "invalid" } },
    { label: "no personal details", mutation: { personalDetails: null } },
    {
      label: "details of another profile",
      mutation: { personalDetails: { ...details, profileId: GRACE_ID } },
    },
  ])(
    "UI-PER-008 refuses a save response with $label and keeps revision 7 and both drafts",
    async ({ mutation }) => {
      const updates: Array<{ expectedRevision: string }> = [];
      editorBackend(adaGraph, {
        update_personal_details: () => ({ ...validSaved, ...mutation }),
        update_profile: (payload) => {
          updates.push(payload as { expectedRevision: string });
          return savedGraph(adaGraph, "8");
        },
      });

      renderPage();
      await openEditorProfile();
      fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
        target: { value: "Unsaved words" },
      });
      fireEvent.change(within(editorSection()).getByTestId("nationalityInput"), {
        target: { value: "Unsaved nationality" },
      });
      fireEvent.click(screen.getByTestId("savePersonalDetailsButton"));

      expect(
        await screen.findByText("Unable to save this profile. Please try again."),
      ).toBeInTheDocument();
      expect(screen.queryByText("Personal details saved.")).not.toBeInTheDocument();
      expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved words");
      expect(within(editorSection()).getByTestId("nationalityInput")).toHaveValue(
        "Unsaved nationality",
      );

      // The loaded revision is still the one a later profile save sends.
      fireEvent.click(screen.getByTestId("saveProfileButton"));
      await waitFor(() => expect(updates).toHaveLength(1));
      expect(updates[0]?.expectedRevision).toBe("7");
    },
  );
});

describe("profile form guidance and inline errors", () => {
  it("explains the form and shows what each field accepts, with an example", async () => {
    editorBackend(graphWith({}));
    renderPage();
    await openEditorProfile();

    const editor = editorSection();
    expect(within(editor).getByTestId("formIntro")).toHaveTextContent(
      "Dates are written as YYYY-MM-DD",
    );
    const email = within(editor).getByTestId("emailInput");
    expect(email).toHaveAccessibleDescription(
      "Required · Email address · Example: ada@example.com",
    );
    expect(within(editor).getByTestId("dateOfBirthInput")).toHaveAccessibleDescription(
      "Optional · Date as YYYY-MM-DD · Example: 1990-04-23. Leave empty to clear it",
    );
  });

  it("marks a wrong email next to its field once it is left, and clears it when fixed", async () => {
    editorBackend(graphWith({}));
    renderPage();
    await openEditorProfile();

    const email = within(editorSection()).getByTestId("emailInput");
    fireEvent.change(email, { target: { value: "ada@example" } });
    expect(email).not.toHaveAttribute("aria-invalid");
    fireEvent.blur(email);

    expect(email).toHaveAttribute("aria-invalid", "true");
    expect(email).toHaveAccessibleDescription(/not a valid email address.*name@example\.com/);

    fireEvent.change(email, { target: { value: "ada@example.com" } });
    expect(email).not.toHaveAttribute("aria-invalid");
  });

  it("refuses to save an invalid form, focuses the first wrong field and sends nothing", async () => {
    const updates: unknown[] = [];
    editorBackend(
      graphWith({
        education: [
          {
            id: "00000000-0000-4000-8000-0000000000f1",
            institution: "Example University",
            degree: null,
            field: null,
            location: null,
            startDate: "2020-01-15",
            endDate: "2019-12-31",
            relevantFocus: null,
          },
        ],
      }),
      {
        update_profile: (payload) => {
          updates.push(payload);
          return savedGraph(graphWith({}), "8");
        },
      },
    );
    renderPage();
    await openEditorProfile();

    const summary = within(editorSection()).getByTestId("summaryInput");
    fireEvent.change(summary, { target: { value: "Edited" } });
    fireEvent.click(screen.getByTestId("saveProfileButton"));

    const endDate = within(editorSection()).getByTestId("education_0_endDateInput");
    expect(endDate).toHaveFocus();
    expect(endDate).toHaveAccessibleDescription(/end date is before the start date/);
    expect(
      screen.getByText("Some profile fields are invalid. Review the form and try again."),
    ).toBeInTheDocument();
    expect(updates).toEqual([]);
  });

  it("marks a required field left empty with an example of what to enter", async () => {
    editorBackend(graphWith({ skills: [] }));
    renderPage();
    await openEditorProfile();

    fireEvent.click(within(editorSection()).getByTestId("addSkillsButton"));
    const skill = within(editorSection()).getByTestId("skills_0_skillInput");
    expect(skill).not.toHaveAttribute("aria-invalid");
    fireEvent.blur(skill);

    expect(skill).toHaveAttribute("aria-invalid", "true");
    expect(skill).toHaveAccessibleDescription(/Skill is required\. Enter a value such as Rust\./);
  });

  it("marks a wrong date of birth and does not send the personal save", async () => {
    const seen: unknown[] = [];
    editorBackend(graphWith({}), {
      update_personal_details: (payload) => {
        seen.push(payload);
        return adaPersonal;
      },
    });
    renderPage();
    await openEditorProfile();

    const date = within(editorSection()).getByTestId("dateOfBirthInput");
    fireEvent.change(date, { target: { value: "23.04.1990" } });
    fireEvent.click(screen.getByTestId("savePersonalDetailsButton"));

    await waitFor(() => expect(date).toHaveAttribute("aria-invalid", "true"));
    expect(date).toHaveAccessibleDescription(/not a valid date.*2023-06-30/);
    expect(seen).toEqual([]);
  });
});

describe("hiding the profile workspace", () => {
  it("hides and shows the workspace without losing the draft, and flags unsaved edits while hidden", async () => {
    editorBackend(graphWith({}));
    renderPage();
    await openEditorProfile();

    const toggle = screen.getByTestId("toggleWorkspaceButton");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.change(within(editorSection()).getByTestId("summaryInput"), {
      target: { value: "Unsaved words" },
    });

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveTextContent("Show workspace");
    expect(screen.getByTestId("profileEditorPanel")).not.toBeVisible();
    expect(screen.getByTestId("hiddenUnsavedMarker")).toBeVisible();

    fireEvent.click(toggle);
    expect(toggle).toHaveTextContent("Hide workspace");
    expect(screen.getByTestId("profileEditorPanel")).toBeVisible();
    expect(screen.queryByTestId("hiddenUnsavedMarker")).not.toBeInTheDocument();
    expect(within(editorSection()).getByTestId("summaryInput")).toHaveValue("Unsaved words");
  });

  it("shows the workspace again when a profile is opened while it is hidden", async () => {
    editorBackend(graphWith({}));
    renderPage();
    fireEvent.click(screen.getByTestId("toggleWorkspaceButton"));
    expect(screen.getByTestId("profileEditorPanel")).not.toBeVisible();

    await openEditorProfile();

    expect(screen.getByTestId("profileEditorPanel")).toBeVisible();
    expect(screen.getByTestId("toggleWorkspaceButton")).toHaveAttribute("aria-expanded", "true");
  });
});
