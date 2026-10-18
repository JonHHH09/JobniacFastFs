import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { InstallKindDto, UpdateCheckDto, UpdateSettingsDto } from "../../ipc/bindings";
import { UpdateBanner } from "./UpdateBanner";
import { UpdatesCard } from "./UpdatesCard";

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => undefined),
}));

function settings(over: Partial<UpdateSettingsDto> = {}): UpdateSettingsDto {
  return { autoCheck: false, supported: true, installKind: "installer", pending: null, ...over };
}

function found(installKind: InstallKindDto = "installer"): UpdateCheckDto {
  return { available: true, version: "9.9.9", notes: "Synthetic notes", installKind };
}

function renderUi() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <UpdateBanner />
      <UpdatesCard />
    </QueryClientProvider>,
  );
}

function mockUpdates(
  initial: UpdateSettingsDto,
  extra: Record<string, (args: Record<string, unknown>) => unknown> = {},
) {
  const invoke = vi.fn((cmd: string, args?: Record<string, unknown>) => {
    const handler = extra[cmd];
    if (handler) {
      return handler(args ?? {});
    }
    if (cmd === "update_settings_get") {
      return initial;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  mockIPC(invoke as Parameters<typeof mockIPC>[0]);
  return invoke;
}

const calls = (invoke: ReturnType<typeof mockUpdates>, name: string) =>
  invoke.mock.calls.filter(([cmd]) => cmd === name);

describe("updates", () => {
  it("shows the auto-check switch off by default and no banner", async () => {
    mockUpdates(settings());
    renderUi();
    const toggle = await screen.findByRole("switch", { name: "Check for updates after startup" });
    expect(toggle).not.toBeChecked();
    expect(screen.queryByTestId("updateBanner")).not.toBeInTheDocument();
  });

  it("saves the auto-check switch through Rust and shows its answer", async () => {
    const invoke = mockUpdates(settings(), {
      update_set_auto_check: () => settings({ autoCheck: true }),
    });
    renderUi();
    await userEvent.click(
      await screen.findByRole("switch", { name: "Check for updates after startup" }),
    );
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: "Check for updates after startup" })).toBeChecked(),
    );
    expect(calls(invoke, "update_set_auto_check")[0]?.[1]).toEqual({ enabled: true });
  });

  it("says updates are unavailable in a build without configuration and offers no check", async () => {
    mockUpdates(settings({ supported: false }));
    renderUi();
    expect(await screen.findByText("Updates are unavailable in this build.")).toBeInTheDocument();
    expect(screen.getByTestId("updateCheckButton")).toBeDisabled();
  });

  it("reports an up-to-date copy after a check", async () => {
    mockUpdates(settings(), {
      check_for_update: () => ({ ...found(), available: false, version: null, notes: null }),
    });
    renderUi();
    await userEvent.click(await screen.findByTestId("updateCheckButton"));
    expect(await screen.findByText("You have the latest version.")).toBeInTheDocument();
  });

  it("shows the banner for a pending startup result and installs only after the confirmation", async () => {
    const invoke = mockUpdates(settings({ pending: found() }), {
      install_update: () => null,
    });
    renderUi();
    const banner = await screen.findByTestId("updateBanner");
    expect(banner).toHaveTextContent("Update available: v9.9.9");
    await userEvent.click(screen.getAllByTestId("updateInstallButton")[0] as HTMLElement);
    expect(calls(invoke, "install_update")).toHaveLength(0);
    await userEvent.click(await screen.findByTestId("updateInstallConfirm"));
    await waitFor(() => expect(calls(invoke, "install_update")).toHaveLength(1));
  });

  it("cancelling the confirmation installs nothing", async () => {
    const invoke = mockUpdates(settings({ pending: found() }));
    renderUi();
    await screen.findByTestId("updateBanner");
    await userEvent.click(screen.getAllByTestId("updateInstallButton")[0] as HTMLElement);
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(calls(invoke, "install_update")).toHaveLength(0);
  });

  it("shows the releases link instead of an install button for a deb copy", async () => {
    mockUpdates(settings({ installKind: "deb", pending: found("deb") }));
    renderUi();
    await screen.findByTestId("updateBanner");
    expect(screen.queryByTestId("updateInstallButton")).not.toBeInTheDocument();
    const links = screen.getAllByRole("link", { name: "Open the releases page" });
    expect(links[0]).toHaveAttribute(
      "href",
      "https://github.com/JonHHH09/JobniacFastFs/releases/latest",
    );
  });

  it("shows the copy of a failed check, not the raw error", async () => {
    mockUpdates(settings(), {
      check_for_update: () => {
        throw { code: "upstream_unavailable", messageId: "update.checkFailed" };
      },
    });
    renderUi();
    await userEvent.click(await screen.findByTestId("updateCheckButton"));
    expect(
      await screen.findByText("Unable to check for updates. Check your connection and try again."),
    ).toBeInTheDocument();
  });
});
