import { type InfiniteData, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import type {
  PersonalDetailsEnvelopeDto,
  ProfileGraphDto,
  ProfilePageDto,
  ProfileSearchDto,
} from "../../ipc/bindings";
import { baselineProject, canSaveProject, draftFromGraph, projectPatchRequest } from "./draft";
import { editorReducer, hasUnsavedChanges, initialEditorState } from "./editorState";
import { ProfilesPage } from "./ProfilesPage";
import { profileKeys, renameProfile } from "./queries";
import { useProfileEditor } from "./useProfileEditor";

// Counts the full-graph comparisons; the real implementation still runs.
const dirtyCalls = vi.hoisted(() => ({ count: 0 }));
vi.mock("./draft", async (importOriginal) => {
  const original = await importOriginal<typeof import("./draft")>();
  return {
    ...original,
    isProfileDirty: (...args: Parameters<typeof original.isProfileDirty>) => {
      dirtyCalls.count += 1;
      return original.isProfileDirty(...args);
    },
  };
});

// Synthetic identifiers and personal data only.
const ADA_ID = "00000000-0000-4000-8000-0000000000a1";
const PROJECT_A = "00000000-0000-4000-8000-0000000000c1";
const PROJECT_B = "00000000-0000-4000-8000-0000000000c2";

function project(id: string, name: string) {
  return { id, name, url: null, description: null, displayOrder: 0, technologies: [] };
}

function graph(collections: Record<string, unknown[]> = {}): ProfileGraphDto {
  return {
    profile: {
      id: ADA_ID,
      fullName: "Ada Example",
      email: "ada@example.test",
      summary: "Analyst",
      revision: "7",
      createdAt: "2026-01-02T03:04:05Z",
      updatedAt: "2026-01-02T03:04:05Z",
    },
    contacts: [],
    links: [],
    skills: [],
    languages: [],
    education: [],
    experiences: [],
    projects: [],
    ...collections,
  } as unknown as ProfileGraphDto;
}

const personal: PersonalDetailsEnvelopeDto = {
  profileId: ADA_ID,
  profileRevision: "7",
  profileUpdatedAt: "2026-01-02T03:04:05Z",
  personalDetails: null,
};

const search: ProfileSearchDto = {
  query: "ada",
  queryTokens: ["ada"],
  totalMatches: "1",
  matchedCount: "1",
  hasMore: false,
  returnedCount: 1,
  profiles: [
    {
      profile: { id: ADA_ID, fullName: "Ada Example", email: "ada@example.test" },
      matchedFields: ["fullName"],
      score: "10",
    },
  ],
};

type Overrides = Record<string, (payload: unknown) => unknown>;

function backend(profileGraph: ProfileGraphDto, overrides: Overrides = {}) {
  mockIPC((cmd, payload) => {
    const override = overrides[cmd];
    if (override) {
      return override(payload) as never;
    }
    switch (cmd) {
      case "deletion_capabilities_get":
        return { deletion: true } as never;
      case "search_profiles":
        return search as never;
      case "get_profile":
        return profileGraph as never;
      case "get_personal_details":
        return personal as never;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ProfilesPage />
    </QueryClientProvider>,
  );
}

async function clickOpenAda() {
  fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
  fireEvent.click(screen.getByTestId("searchProfilesButton"));
  fireEvent.click(await screen.findByRole("button", { name: "Open profile Ada Example" }));
}

async function openAda() {
  await clickOpenAda();
  expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
}

describe("opening a profile (S000, S002)", () => {
  it("never runs the graph and personal reads at the same time", async () => {
    let inFlight = 0;
    let overlapped = false;
    const guarded = (result: unknown) => async () => {
      // Rust refuses a second profile command while one holds the single-flight slot.
      if (inFlight > 0) {
        overlapped = true;
        throw { code: "busy", messageId: "profile.busy" };
      }
      inFlight += 1;
      await new Promise((resolve) => setTimeout(resolve, 5));
      inFlight -= 1;
      return result;
    };
    backend(graph(), {
      get_profile: guarded(graph()),
      get_personal_details: guarded(personal),
    });

    renderPage();
    await openAda();

    expect(overlapped).toBe(false);
  });

  it("shows the loading text, not the previous status, while a profile loads (H010)", async () => {
    backend(graph(), { get_profile: () => new Promise(() => undefined) });

    renderPage();
    await clickOpenAda();

    await waitFor(() =>
      expect(screen.getByTestId("profileMessage")).toHaveTextContent("Loading profile…"),
    );
    expect(screen.getByTestId("saveProfileButton")).toHaveTextContent("Save profile");
  });
});

describe("project rows are matched by id (S001)", () => {
  const twoProjects = graph({
    projects: [project(PROJECT_A, "Alpha"), project(PROJECT_B, "Beta")],
  });

  it("does not offer a project save for a row whose baseline is another row", () => {
    const baseline = draftFromGraph(twoProjects);
    const draft = {
      ...baseline,
      collections: {
        ...baseline.collections,
        projects: [baseline.collections.projects?.[1] ?? {}],
      },
    };

    expect(baselineProject(baseline, draft.collections.projects?.[0])?.id).toBe(PROJECT_B);
    expect(canSaveProject(draft, baseline, 0)).toBe(false);
  });

  it("sends the id and the changed fields of the same project", () => {
    const baseline = draftFromGraph(twoProjects);
    const second = { ...(baseline.collections.projects?.[1] ?? {}), name: "Beta 2" };
    const draft = { ...baseline, collections: { ...baseline.collections, projects: [second] } };

    expect(canSaveProject(draft, baseline, 0)).toBe(true);
    expect(projectPatchRequest(ADA_ID, draft, baseline, 0, "7")).toEqual({
      profileId: ADA_ID,
      projectId: PROJECT_B,
      expectedRevision: "7",
      name: "Beta 2",
    });
  });

  it("updates the saved project by id in the draft and the baseline", () => {
    const loaded = editorReducer(initialEditorState, {
      type: "loaded",
      graph: twoProjects,
      personal,
    });
    const removed = editorReducer(loaded, { type: "removeRow", collection: "projects", index: 0 });
    const state = editorReducer(removed, {
      type: "projectSaved",
      project: project(PROJECT_B, "Beta 2"),
      profileRevision: "8",
    });

    expect(state.draft?.collections.projects?.map((row) => row.name)).toEqual(["Beta 2"]);
    expect(state.baseline?.collections.projects?.map((row) => row.name)).toEqual([
      "Alpha",
      "Beta 2",
    ]);
  });
});

describe("discarding a new profile (S011, H078)", () => {
  it("returns to the blank form and reports a clean draft", () => {
    let state = editorReducer(initialEditorState, { type: "newProfile" });
    state = editorReducer(state, { type: "setField", field: "fullName", value: "Typed" });
    expect(hasUnsavedChanges(state)).toBe(true);

    state = editorReducer(state, { type: "discard" });

    expect(state.mode).toBe("new");
    expect(state.draft?.fullName).toBe("");
    expect(hasUnsavedChanges(state)).toBe(false);
  });
});

describe("a late delete token (S014)", () => {
  it("does not open the dialog or reset the editor after the user started a new profile", async () => {
    let release: (token: { confirmationToken: string }) => void = () => undefined;
    backend(graph(), {
      request_profile_delete: () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    });

    renderPage();
    await openAda();
    fireEvent.click(await screen.findByTestId("deleteProfileButton"));
    await waitFor(() => expect(screen.getByTestId("deleteProfileButton")).toBeDisabled());
    fireEvent.click(screen.getByTestId("newProfileButton"));
    fireEvent.change(screen.getByTestId("fullNameInput"), { target: { value: "Typed" } });

    await act(async () => {
      release({ confirmationToken: "token-1" });
    });

    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByTestId("fullNameInput")).toHaveValue("Typed");
  });

  it("keeps the editor when a deletion confirms for another profile", () => {
    const loaded = editorReducer(initialEditorState, { type: "loaded", graph: graph(), personal });
    const next = editorReducer(loaded, {
      type: "deleted",
      profileId: "other",
      message: "profile.deleted",
    });

    expect(next.profileId).toBe(ADA_ID);
    expect(next.message).toBe("profile.deleted");
  });
});

describe("cached rows after a save (H077)", () => {
  it("patches the list and the search results with the saved name", () => {
    const client = new QueryClient();
    const row = { id: ADA_ID, fullName: "Old", email: "old@example.test" };
    const list: InfiniteData<ProfilePageDto, string | null> = {
      pages: [{ profiles: [row], nextCursor: null }],
      pageParams: [null],
    };
    client.setQueryData(profileKeys.list, list);
    client.setQueryData(profileKeys.search("ada"), {
      ...search,
      profiles: [{ ...search.profiles[0], profile: row }],
    });

    renameProfile(client, { id: ADA_ID, fullName: "New", email: "new@example.test" });

    const patched = client.getQueryData<InfiniteData<ProfilePageDto>>(profileKeys.list);
    expect(patched?.pages[0].profiles[0]).toEqual({
      id: ADA_ID,
      fullName: "New",
      email: "new@example.test",
    });
    const found = client.getQueryData<ProfileSearchDto>(profileKeys.search("ada"));
    expect(found?.profiles[0].profile.fullName).toBe("New");
  });
});

describe("dirty state cost (H079, S029)", () => {
  it("compares the full graphs once per draft change", () => {
    const client = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(() => useProfileEditor(), { wrapper });
    act(() => result.current.dispatch({ type: "loaded", graph: graph(), personal }));

    dirtyCalls.count = 0;
    act(() =>
      result.current.dispatch({ type: "setField", field: "summary", value: "Edited summary" }),
    );
    expect(result.current.dirty).toBe(true);
    expect(dirtyCalls.count).toBe(1);

    rerender();
    expect(dirtyCalls.count).toBe(1);
  });
});

describe("remove buttons name their row (H011)", () => {
  it("gives each row's Remove button a different accessible name", async () => {
    const experience = (id: string, company: string) => ({
      id,
      company,
      title: "Engineer",
      location: null,
      startDate: "2020-01-01",
      endDate: null,
      description: null,
      displayOrder: 0,
    });
    backend(
      graph({
        experiences: [
          experience("00000000-0000-4000-8000-0000000000e1", "Example Labs"),
          experience("00000000-0000-4000-8000-0000000000e2", "Sample Works"),
        ],
      }),
    );

    renderPage();
    await openAda();

    expect(screen.getByTestId("removeExperiences0Button")).toHaveAccessibleName(
      /^Remove entry 1: .*Example Labs/,
    );
    expect(screen.getByTestId("removeExperiences1Button")).toHaveAccessibleName(
      /^Remove entry 2: .*Sample Works/,
    );
  });
});
