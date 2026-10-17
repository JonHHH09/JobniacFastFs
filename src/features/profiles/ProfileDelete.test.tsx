import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  PersonalDetailsEnvelopeDto,
  ProfileGraphDto,
  ProfileSearchDto,
} from "../../ipc/bindings";
import { ProfilesPage } from "./ProfilesPage";

// Synthetic identifiers and personal data only.
const ADA_ID = "00000000-0000-4000-8000-0000000000a1";

const ADA_GRAPH: ProfileGraphDto = {
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
};

const ADA_PERSONAL: PersonalDetailsEnvelopeDto = {
  profileId: ADA_ID,
  profileRevision: "7",
  profileUpdatedAt: "2026-01-02T03:04:05Z",
  personalDetails: null,
};

const ADA_SEARCH: ProfileSearchDto = {
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

/** A backend for the profile screens. Unknown commands fail, so a stray call is visible. */
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
    if (cmd === "search_profiles") {
      return ADA_SEARCH as never;
    }
    if (cmd === "get_profile") {
      return ADA_GRAPH as never;
    }
    if (cmd === "get_personal_details") {
      return ADA_PERSONAL as never;
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
      <ProfilesPage />
    </QueryClientProvider>,
  );
}

/** Opens Ada from her search result and waits until the editor shows the loaded profile. */
async function openAda() {
  fireEvent.change(screen.getByTestId("profileSearchInput"), { target: { value: "ada" } });
  fireEvent.click(screen.getByTestId("searchProfilesButton"));
  fireEvent.click(await screen.findByRole("button", { name: "Open profile Ada Example" }));
  expect(await screen.findByText("Profile loaded.")).toBeInTheDocument();
}

/** Opens the delete confirmation and waits until the dialog shows. */
async function openDeleteDialog() {
  fireEvent.click(await screen.findByTestId("deleteProfileButton"));
  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
}

describe("profile delete: availability", () => {
  it("hides the delete control when the deletion service is not composed", async () => {
    const calls = backend(false);
    renderPage();
    await openAda();
    await waitFor(() =>
      expect(calls.some(({ cmd }) => cmd === "deletion_capabilities_get")).toBe(true),
    );
    await waitFor(() => expect(screen.queryByTestId("deleteProfileButton")).toBeNull());
  });

  it("asks the capability only once a profile is loaded", async () => {
    const calls = backend(true);
    renderPage();
    expect(calls.some(({ cmd }) => cmd === "deletion_capabilities_get")).toBe(false);
    await openAda();
    expect(await screen.findByTestId("deleteProfileButton")).toBeEnabled();
  });
});

describe("profile delete: two steps", () => {
  it("requests a token for the loaded profile and shows the named confirmation", async () => {
    const calls = backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    const requests = calls.filter(({ cmd }) => cmd === "request_profile_delete");
    expect(requests.map(({ payload }) => payload)).toEqual([{ profileId: ADA_ID }]);
    expect(calls.some(({ cmd }) => cmd === "delete_profile")).toBe(false);
    expect(screen.getByTestId("deleteProfileDialogMessage").textContent).toContain(
      'Delete profile "Ada Example"? This permanently removes the profile',
    );
  });

  it("Cancel keeps the profile and never calls the delete", async () => {
    const calls = backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    fireEvent.click(screen.getByTestId("deleteProfileDialogCancelButton"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(calls.some(({ cmd }) => cmd === "delete_profile")).toBe(false);
    expect(screen.getByTestId("fullNameInput")).toHaveValue("Ada Example");
  });

  it("Escape cancels the confirmation without deleting", async () => {
    const calls = backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(calls.some(({ cmd }) => cmd === "delete_profile")).toBe(false);
  });

  it("confirm sends the echoed token, clears the editor and drops the search row", async () => {
    const calls = backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
      delete_profile: () => ({ profileId: ADA_ID, deleted: true }),
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    expect(await screen.findByText("Profile deleted.")).toBeInTheDocument();
    expect(
      calls.filter(({ cmd }) => cmd === "delete_profile").map(({ payload }) => payload),
    ).toEqual([{ profileId: ADA_ID, confirmationToken: "token-1" }]);
    expect(screen.queryByTestId("fullNameInput")).toBeNull();
    expect(screen.queryByRole("button", { name: "Open profile Ada Example" })).toBeNull();
  });

  it("a profile that no longer exists is removed with its own copy", async () => {
    backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
      delete_profile: () => ({ profileId: ADA_ID, deleted: false }),
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    expect(
      await screen.findByText("This profile no longer exists. Nothing was deleted."),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("fullNameInput")).toBeNull();
  });
});

describe("profile delete: failures", () => {
  it("shows the fixed failure copy and keeps the profile loaded", async () => {
    backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
      delete_profile: () => {
        throw { code: "validation_error", messageId: "delete.failed.profile" };
      },
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    expect(
      await screen.findByText(
        "Could not confirm the deletion of this profile. Reload to check whether it was removed.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByTestId("fullNameInput")).toHaveValue("Ada Example");
  });

  it("shows the generation copy for a conflict", async () => {
    backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
      delete_profile: () => {
        throw { code: "conflict", messageId: "delete.failed.conflict" };
      },
    });
    renderPage();
    await openAda();
    await openDeleteDialog();

    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    expect(
      await screen.findByText("A generation is in progress. Try again later."),
    ).toBeInTheDocument();
  });

  it("reopens the same confirmation with its token when the call is refused as busy", async () => {
    const tokens: unknown[] = [];
    backend(true, {
      request_profile_delete: () => ({ confirmationToken: "token-1" }),
      delete_profile: (payload) => {
        tokens.push((payload as { confirmationToken: unknown }).confirmationToken);
        throw { code: "busy", messageId: "profile.busy" };
      },
    });
    renderPage();
    await openAda();
    await openDeleteDialog();
    const message = screen.getByTestId("deleteProfileDialogMessage").textContent;

    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    await waitFor(() => expect(tokens).toHaveLength(1));
    // The dialog is open again with the same text; no failure copy and the editor keeps its input.
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(screen.getByTestId("deleteProfileDialogMessage").textContent).toBe(message);
    expect(screen.queryByText(/Could not confirm the deletion/)).toBeNull();
    expect(screen.getByTestId("fullNameInput")).toHaveValue("Ada Example");

    // The kept token is echoed again on the retry.
    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    await waitFor(() => expect(tokens).toEqual(["token-1", "token-1"]));
  });

  it("does not open the dialog when Rust refuses the request", async () => {
    const onError = vi.fn();
    backend(true, {
      request_profile_delete: () => {
        onError();
        throw { code: "not_configured", messageId: "delete.failed.profile" };
      },
    });
    renderPage();
    await openAda();
    fireEvent.click(await screen.findByTestId("deleteProfileButton"));
    expect(
      await screen.findByText(
        "Could not confirm the deletion of this profile. Reload to check whether it was removed.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(onError).toHaveBeenCalledTimes(1);
  });
});
