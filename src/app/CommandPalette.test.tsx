import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { requestOpen, resetOpenRequests, subscribeOpenRequests } from "./openRequests";
import { createAppRouter } from "./router";

// As in AppShell.test.tsx: no Rust, small stand-ins for the section pages.
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => undefined),
}));
vi.mock("../ipc/commands", () => ({ runCommand: () => Promise.resolve(null) }));
vi.mock("../features/profiles/ProfilesPage", () => ({
  ProfilesPage: () => <input aria-label="Draft name" />,
}));
vi.mock("../features/jobs/JobsPage", () => ({
  JobsPage: () => <p>Jobs stand-in</p>,
}));

const api = vi.hoisted(() => ({
  fetchProfilePage: vi.fn(),
  searchProfiles: vi.fn(),
  fetchJobPage: vi.fn(),
  searchJobs: vi.fn(),
}));
vi.mock("../features/profiles/api", () => ({
  fetchProfilePage: api.fetchProfilePage,
  searchProfiles: api.searchProfiles,
  fetchProfile: vi.fn(),
}));
vi.mock("../features/jobs/api", () => ({
  fetchJobPage: api.fetchJobPage,
  searchJobs: api.searchJobs,
  fetchJob: vi.fn(),
}));

const ADA = "00000000-0000-4000-8000-0000000000a1";
const JOB = "00000000-0000-4000-8000-0000000000b1";

async function renderShell(path = "/") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createAppRouter(createMemoryHistory({ initialEntries: [path] }));
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("navigation", { name: "Main navigation" });
  return router;
}

function palette(): HTMLElement | null {
  return screen.queryByRole("dialog", { name: "Command palette" });
}

beforeEach(() => {
  api.searchProfiles.mockResolvedValue({
    profiles: [
      {
        profile: { id: ADA, fullName: "Ada Lovelace", email: "ada@example.com" },
        matchedFields: ["fullName"],
        score: "1",
      },
    ],
  });
  api.searchJobs.mockResolvedValue({
    jobs: [
      {
        job: {
          id: JOB,
          title: "Adaptive Systems Engineer",
          company: "Example Co",
          location: "Berlin",
        },
      },
    ],
  });
});

afterEach(() => {
  vi.clearAllMocks();
  resetOpenRequests();
  for (const stray of document.querySelectorAll("[data-stray-dialog]")) {
    stray.remove();
  }
});

describe("command palette", () => {
  it("shows the shortcut as a key cap hint in the sidebar footer", async () => {
    await renderShell();
    const sidebar = screen.getByTestId("sidebar");
    const button = within(sidebar).getByRole("button", { name: "Open command palette" });
    expect(button).toHaveAttribute("aria-keyshortcuts", "Control+K Meta+K");
    expect(button.querySelectorAll("kbd")).toHaveLength(2);
    expect(button.querySelector("kbd:last-child")).toHaveTextContent("K");
  });

  it("opens with Ctrl+K, lists the sections and closes with Escape", async () => {
    const user = userEvent.setup();
    await renderShell();
    await screen.findByLabelText("Draft name");
    expect(palette()).not.toBeInTheDocument();
    await user.keyboard("{Control>}k{/Control}");
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    const input = within(dialog).getByRole("combobox", { name: "Command palette" });
    expect(input).toHaveFocus();
    expect(within(dialog).getAllByRole("option")).toHaveLength(11);
    expect(within(dialog).getByRole("option", { name: /Settings/ })).toBeInTheDocument();
    expect(within(dialog).getByRole("option", { name: /Saved jobs/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(palette()).not.toBeInTheDocument();
  });

  it("opens with Cmd+K as well", async () => {
    const user = userEvent.setup();
    await renderShell();
    await screen.findByLabelText("Draft name");
    await user.keyboard("{Meta>}k{/Meta}");
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });

  it("opens from the sidebar button", async () => {
    const user = userEvent.setup();
    await renderShell();
    await user.click(
      within(screen.getByTestId("sidebar")).getByRole("button", { name: "Open command palette" }),
    );
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });

  it("toggles closed with Ctrl+K from inside the palette", async () => {
    const user = userEvent.setup();
    await renderShell();
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByRole("dialog", { name: "Command palette" });
    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).not.toBeInTheDocument();
  });

  it("does not open while the user types in a field", async () => {
    const user = userEvent.setup();
    await renderShell();
    const draft = await screen.findByLabelText("Draft name");
    await user.click(draft);
    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).not.toBeInTheDocument();
    expect(draft).toHaveFocus();
  });

  it("does not open while another dialog is open", async () => {
    const user = userEvent.setup();
    await renderShell();
    await screen.findByLabelText("Draft name");
    const stray = document.createElement("div");
    stray.setAttribute("role", "alertdialog");
    stray.setAttribute("data-stray-dialog", "");
    document.body.append(stray);
    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).not.toBeInTheDocument();
  });

  it("ignores Ctrl+Shift+K and Alt+Ctrl+K", async () => {
    const user = userEvent.setup();
    await renderShell();
    await screen.findByLabelText("Draft name");
    await user.keyboard("{Control>}{Shift>}k{/Shift}{/Control}");
    await user.keyboard("{Control>}{Alt>}k{/Alt}{/Control}");
    expect(palette()).not.toBeInTheDocument();
  });

  it("filters the sections by what is typed and goes to the chosen one", async () => {
    const user = userEvent.setup();
    const router = await renderShell();
    await screen.findByLabelText("Draft name");
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("saved");
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    expect(within(dialog).getByRole("option", { name: /Saved jobs/ })).toBeInTheDocument();
    expect(within(dialog).queryByRole("option", { name: /Health/ })).not.toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(await screen.findByText("Jobs stand-in")).toBeVisible();
    expect(router.state.location.search).toEqual({ section: "jobs" });
    expect(palette()).not.toBeInTheDocument();
  });

  it("searches profiles and jobs after the debounce and groups the results", async () => {
    const user = userEvent.setup();
    await renderShell();
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("ada");
    expect(api.searchProfiles).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    await within(dialog).findByRole("option", { name: /Ada Lovelace/ });
    await within(dialog).findByRole("option", { name: /Adaptive Systems Engineer/ });
    expect(api.searchProfiles).toHaveBeenCalledTimes(1);
    expect(api.searchProfiles).toHaveBeenCalledWith("ada");
    expect(api.searchJobs).toHaveBeenCalledWith("ada");
    expect(within(dialog).getByText("Profiles")).toBeInTheDocument();
    expect(within(dialog).getByText("Saved jobs", { selector: "div" })).toBeInTheDocument();
  });

  it("opens a chosen profile: navigates to its section and asks it to open the record", async () => {
    const user = userEvent.setup();
    const router = await renderShell("/?section=jobs");
    await screen.findByText("Jobs stand-in");
    const opened = vi.fn();
    subscribeOpenRequests("profile", opened);
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("ada");
    await user.click(await screen.findByRole("option", { name: /Ada Lovelace/ }));
    expect(opened).toHaveBeenCalledExactlyOnceWith({ kind: "profile", id: ADA });
    await waitFor(() => expect(router.state.location.search).toEqual({ section: "profiles" }));
    expect(palette()).not.toBeInTheDocument();
  });

  it("keeps a chosen job waiting for a section that has not mounted yet", async () => {
    const user = userEvent.setup();
    await renderShell();
    await screen.findByLabelText("Draft name");
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("ada");
    await user.click(await screen.findByRole("option", { name: /Adaptive Systems Engineer/ }));
    const opened = vi.fn();
    act(() => {
      subscribeOpenRequests("job", opened);
    });
    expect(opened).toHaveBeenCalledExactlyOnceWith({ kind: "job", id: JOB });
  });

  it("says when nothing matches and when a search failed", async () => {
    api.searchProfiles.mockRejectedValue({ code: "internal_error", messageId: "" });
    api.searchJobs.mockResolvedValue({ jobs: [] });
    const user = userEvent.setup();
    await renderShell();
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("zzqq");
    expect(await screen.findByText("Profiles could not be searched.")).toBeInTheDocument();
    expect(screen.getByText("Nothing matches “zzqq”.")).toBeInTheDocument();
  });

  it("does not search for a pasted id", async () => {
    const user = userEvent.setup();
    await renderShell();
    await user.keyboard("{Control>}k{/Control}");
    await user.paste(ADA);
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(api.searchProfiles).not.toHaveBeenCalled();
    expect(api.searchJobs).not.toHaveBeenCalled();
  });

  it("lets requestOpen reach a subscriber outside the palette too", () => {
    const opened = vi.fn();
    subscribeOpenRequests("job", opened);
    requestOpen({ kind: "job", id: JOB });
    expect(opened).toHaveBeenCalledWith({ kind: "job", id: JOB });
  });
});
