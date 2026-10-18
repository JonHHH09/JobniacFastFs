import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setShellDirty } from "../features/shell/closeGate";
import { createAppRouter } from "./router";

// The shell never reaches Rust in these tests: the event bridge and the heavy feature pages are
// replaced by small stand-ins, so the tests cover the navigation only.
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
vi.mock("../features/health/HealthPage", () => ({
  HealthPage: () => <h1>Application health</h1>,
}));

async function renderShell(path = "/") {
  const router = createAppRouter(createMemoryHistory({ initialEntries: [path] }));
  // The update banner reads the updater state through React Query (a stand-in answers null).
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("navigation", { name: "Main navigation" });
  return router;
}

function navItem(name: string | RegExp): HTMLElement {
  return within(screen.getByRole("navigation", { name: "Main navigation" })).getByRole("button", {
    name,
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("section navigation shell", () => {
  it("opens the default section and marks it as the current page", async () => {
    await renderShell();
    expect(await screen.findByLabelText("Draft name")).toBeVisible();
    expect(navItem("Profiles")).toHaveAttribute("aria-current", "page");
    expect(navItem("Saved jobs")).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("heading", { level: 1, name: "Profiles" })).toBeInTheDocument();
  });

  it("falls back to the default section for an unknown ?section value", async () => {
    await renderShell("/?section=nonsense");
    expect(await screen.findByLabelText("Draft name")).toBeVisible();
    expect(navItem("Profiles")).toHaveAttribute("aria-current", "page");
  });

  it("opens the section named by ?section", async () => {
    await renderShell("/?section=jobs");
    expect(await screen.findByText("Jobs stand-in")).toBeVisible();
    expect(navItem("Saved jobs")).toHaveAttribute("aria-current", "page");
    expect(screen.queryByLabelText("Draft name")).not.toBeInTheDocument();
  });

  it("switches section by clicking the navigation and updates the URL", async () => {
    const user = userEvent.setup();
    const router = await renderShell();
    await user.click(navItem("Saved jobs"));
    expect(await screen.findByText("Jobs stand-in")).toBeVisible();
    expect(navItem("Saved jobs")).toHaveAttribute("aria-current", "page");
    expect(navItem("Profiles")).not.toHaveAttribute("aria-current");
    expect(router.state.location.search).toEqual({ section: "jobs" });
  });

  it("switches section with Ctrl+2 and moves focus to the section heading", async () => {
    const user = userEvent.setup();
    await renderShell();
    await screen.findByLabelText("Draft name");
    await user.keyboard("{Control>}2{/Control}");
    expect(await screen.findByText("Profile from PDF", { selector: "h1" })).toBeInTheDocument();
    expect(navItem("Profile from PDF")).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { level: 1, name: "Profile from PDF" })).toHaveFocus();
  });

  it("does not jump while the user types in a field", async () => {
    const user = userEvent.setup();
    await renderShell();
    const field = await screen.findByLabelText("Draft name");
    await user.click(field);
    await user.keyboard("{Control>}3{/Control}");
    expect(navItem("Profiles")).toHaveAttribute("aria-current", "page");
  });

  it("mounts a section on its first visit only, then keeps it alive with its draft", async () => {
    const user = userEvent.setup();
    await renderShell();
    const field = await screen.findByLabelText("Draft name");
    // Lazy mount: the jobs section is not in the document before the first visit.
    expect(screen.queryByTestId("section-jobs")).not.toBeInTheDocument();
    expect(screen.queryByText("Jobs stand-in")).not.toBeInTheDocument();

    await user.type(field, "Ada draft");
    await user.click(navItem("Saved jobs"));
    await screen.findByText("Jobs stand-in");
    // Keep-alive: the profiles section is hidden, not unmounted.
    expect(screen.getByTestId("section-profiles")).not.toBeVisible();
    expect(screen.getByTestId("section-jobs")).toBeVisible();

    await user.click(navItem("Profiles"));
    expect(screen.getByLabelText("Draft name")).toHaveValue("Ada draft");
    expect(screen.getByLabelText("Draft name")).toBeVisible();
    expect(screen.getByTestId("section-jobs")).not.toBeVisible();
  });

  it("moves between navigation items with the arrow keys", async () => {
    const user = userEvent.setup();
    await renderShell();
    navItem("Profiles").focus();
    await user.keyboard("{ArrowDown}");
    expect(navItem("Profile from PDF")).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(navItem("Settings")).toHaveFocus();
  });

  it("marks a navigation item while its workspace holds unsaved edits", async () => {
    await renderShell();
    expect(navItem(/^Profiles/)).not.toHaveTextContent("unsaved changes");
    await act(async () => {
      await setShellDirty("profile", true);
    });
    expect(navItem(/^Profiles/)).toHaveTextContent("unsaved changes");
    expect(navItem(/^Saved jobs/)).not.toHaveTextContent("unsaved changes");
    await act(async () => {
      await setShellDirty("profile", false);
    });
    expect(navItem(/^Profiles/)).not.toHaveTextContent("unsaved changes");
  });
});
