import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { McpServerStatusDto, SettingsDto } from "../../ipc/bindings";
import { SettingsPage } from "./SettingsPage";
import { RUNNING_POLL_MS } from "./useMcpServer";

function settings(status: McpServerStatusDto, port = 8765, enabled = status !== "stopped") {
  return { mcp: { enabled, port, status } } satisfies SettingsDto;
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <SettingsPage />
    </QueryClientProvider>,
  );
}

/** Mocks the four commands; `extra` handles or overrides any of them. */
function mockSettings(
  initial: SettingsDto,
  extra: Record<string, (args: Record<string, unknown>) => unknown> = {},
) {
  const invoke = vi.fn((cmd: string, args?: Record<string, unknown>) => {
    const handler = extra[cmd];
    if (handler) {
      return handler(args ?? {});
    }
    if (cmd === "settings_get") {
      return initial;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  mockIPC(invoke as Parameters<typeof mockIPC>[0]);
  return invoke;
}

const calls = (invoke: ReturnType<typeof mockSettings>, name: string) =>
  invoke.mock.calls.filter(([cmd]) => cmd === name);

describe("SettingsPage", () => {
  it.each([
    ["stopped", "Stopped"],
    ["starting", "Starting…"],
    ["running", "Running"],
    ["failed", "Failed"],
    ["unavailable", "Unavailable"],
  ] as const)("renders the %s status", async (status, text) => {
    mockSettings(settings(status));
    renderPage();
    const badge = await screen.findByTestId("mcpStatus");
    expect(badge).toHaveTextContent(text);
    expect(badge).toHaveAttribute("data-status", status);
  });

  it("explains a missing sidecar", async () => {
    mockSettings(settings("unavailable"));
    renderPage();
    expect(await screen.findByText(/cargo build -p jobniac-cli/)).toBeInTheDocument();
  });

  it("turns the server on, locks the controls meanwhile and shows the returned status", async () => {
    let finish: (value: unknown) => void = () => undefined;
    const invoke = mockSettings(settings("stopped"), {
      mcp_server_set_enabled: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    renderPage();
    const toggle = await screen.findByRole("switch", { name: "Run the MCP server" });
    await userEvent.click(toggle);

    await waitFor(() => expect(calls(invoke, "mcp_server_set_enabled")).toHaveLength(1));
    expect(calls(invoke, "mcp_server_set_enabled")[0]?.[1]).toEqual({ enabled: true });
    expect(screen.getByRole("switch", { name: "Run the MCP server" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Run the MCP server" })).toBeChecked();
    expect(screen.getByLabelText("Port")).toBeDisabled();

    finish(settings("running"));
    expect(await screen.findByText("Running")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Run the MCP server" })).toBeEnabled();
  });

  it("shows a failed start as the failed status", async () => {
    mockSettings(settings("stopped"), {
      mcp_server_set_enabled: () => settings("failed"),
    });
    renderPage();
    await userEvent.click(await screen.findByRole("switch", { name: "Run the MCP server" }));
    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Run the MCP server" })).toBeChecked();
  });

  it("validates the port in the form and sends only a valid one", async () => {
    const invoke = mockSettings(settings("stopped"), {
      mcp_server_set_port: (args) => settings("stopped", Number(args.port)),
    });
    renderPage();
    const input = await screen.findByLabelText("Port");
    const save = screen.getByRole("button", { name: "Save port" });
    expect(save).toBeDisabled();

    await userEvent.clear(input);
    await userEvent.type(input, "80a");
    expect(input).toHaveValue("80");
    expect(await screen.findByText("Enter a port from 1024 to 65535.")).toBeInTheDocument();
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(save).toBeDisabled();
    await userEvent.type(input, "{Enter}");
    expect(calls(invoke, "mcp_server_set_port")).toHaveLength(0);

    await userEvent.clear(input);
    await userEvent.type(input, "70000");
    expect(screen.getByText("Enter a port from 1024 to 65535.")).toBeInTheDocument();

    await userEvent.clear(input);
    await userEvent.type(input, "9000{Enter}");
    await waitFor(() => expect(calls(invoke, "mcp_server_set_port")).toHaveLength(1));
    expect(calls(invoke, "mcp_server_set_port")[0]?.[1]).toEqual({ port: 9000 });
    await waitFor(() => expect(screen.getByLabelText("Port")).toHaveValue("9000"));
    expect(screen.queryByText("Enter a port from 1024 to 65535.")).not.toBeInTheDocument();
  });

  it("saves the port with the Save button", async () => {
    const invoke = mockSettings(settings("stopped"), {
      mcp_server_set_port: (args) => settings("stopped", Number(args.port)),
    });
    renderPage();
    const input = await screen.findByLabelText("Port");
    await userEvent.clear(input);
    await userEvent.type(input, "8800");
    await userEvent.click(screen.getByRole("button", { name: "Save port" }));
    await waitFor(() => expect(calls(invoke, "mcp_server_set_port")).toHaveLength(1));
  });

  it.each(["running", "starting"] as const)("disables the port field while %s", async (status) => {
    mockSettings(settings(status));
    renderPage();
    expect(await screen.findByLabelText("Port")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save port" })).toBeDisabled();
  });

  it("keeps the port field editable while stopped", async () => {
    mockSettings(settings("stopped"));
    renderPage();
    expect(await screen.findByLabelText("Port")).toBeEnabled();
  });

  it("offers Copy connection details only while running and copies from Rust", async () => {
    const invoke = mockSettings(settings("running"), { mcp_server_copy_connection: () => null });
    renderPage();
    const button = await screen.findByRole("button", { name: "Copy connection details" });
    await userEvent.click(button);
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(calls(invoke, "mcp_server_copy_connection")).toHaveLength(1);
  });

  it.each(["stopped", "starting", "failed", "unavailable"] as const)(
    "has no copy button while %s",
    async (status) => {
      mockSettings(settings(status));
      renderPage();
      await screen.findByTestId("mcpStatus");
      expect(screen.queryByRole("button", { name: /Copy connection/ })).not.toBeInTheDocument();
    },
  );

  it("never puts a token in the page", async () => {
    mockSettings(settings("running"), { mcp_server_copy_connection: () => null });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Copy connection details" }));
    await screen.findByRole("button", { name: "Copied" });
    const text = document.body.textContent ?? "";
    expect(text).not.toMatch(/[A-Za-z0-9_-]{32,}/);
    expect(text).not.toMatch(/bearer/i);
    expect(document.body.innerHTML).not.toMatch(/127\.0\.0\.1/);
  });

  it("shows catalogue text for a command error, never the raw one", async () => {
    mockSettings(settings("stopped"), {
      mcp_server_set_enabled: () => {
        throw { code: "internal_error", messageId: "settings.mcp.failed" };
      },
    });
    renderPage();
    await userEvent.click(await screen.findByRole("switch", { name: "Run the MCP server" }));
    expect(
      await screen.findByText("The MCP server command could not run. Please try again."),
    ).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Run the MCP server" })).not.toBeChecked();
  });

  it("falls back to operation text for a raw rejection", async () => {
    mockSettings(settings("running"), {
      mcp_server_copy_connection: () => {
        throw "raw secret text from the runtime";
      },
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Copy connection details" }));
    expect(await screen.findByText("Unable to copy to the clipboard.")).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("raw secret");
  });

  it("shows no copy for a busy refusal", async () => {
    mockSettings(settings("stopped"), {
      mcp_server_set_enabled: () => {
        throw { code: "busy", messageId: "settings.busy" };
      },
    });
    renderPage();
    await userEvent.click(await screen.findByRole("switch", { name: "Run the MCP server" }));
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: "Run the MCP server" })).toBeEnabled(),
    );
    expect(screen.queryByText(/Unable|could not/)).not.toBeInTheDocument();
  });

  it("polls while running so a dead server clears the badge and Copy", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let current = settings("running");
      mockSettings(current, { settings_get: () => current });
      renderPage();
      expect(await screen.findByRole("button", { name: "Copy connection details" })).toBeVisible();
      current = settings("failed");
      await vi.advanceTimersByTimeAsync(RUNNING_POLL_MS + 100);
      expect(await screen.findByText("Failed")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /Copy connection/ })).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("refetches the settings when the copy fails and drops the error with the status", async () => {
    let current = settings("running");
    const invoke = mockSettings(current, {
      settings_get: () => current,
      mcp_server_copy_connection: () => {
        current = settings("failed");
        throw { code: "not_configured", messageId: "settings.mcp.notRunning" };
      },
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Copy connection details" }));
    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(calls(invoke, "settings_get").length).toBeGreaterThan(1);
    expect(screen.queryByText("The MCP server is not running.")).not.toBeInTheDocument();
  });

  it("announces the status in a polite live region", async () => {
    mockSettings(settings("stopped"));
    renderPage();
    const badge = await screen.findByTestId("mcpStatus");
    expect(badge).toHaveAttribute("role", "status");
    expect(badge).toHaveAttribute("aria-live", "polite");
  });

  it("keeps a typed port draft when the switch is toggled", async () => {
    mockSettings(settings("stopped"), { mcp_server_set_enabled: () => settings("failed") });
    renderPage();
    const input = await screen.findByLabelText("Port");
    await userEvent.clear(input);
    await userEvent.type(input, "9100");
    await userEvent.click(screen.getByRole("switch", { name: "Run the MCP server" }));
    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(screen.getByLabelText("Port")).toHaveValue("9100");
  });

  it("drops the draft when Rust answers with another port", async () => {
    mockSettings(settings("stopped"), { mcp_server_set_enabled: () => settings("failed", 8000) });
    renderPage();
    const input = await screen.findByLabelText("Port");
    await userEvent.clear(input);
    await userEvent.type(input, "9100");
    await userEvent.click(screen.getByRole("switch", { name: "Run the MCP server" }));
    await waitFor(() => expect(screen.getByLabelText("Port")).toHaveValue("8000"));
  });

  it("shows a load error with a retry", async () => {
    let attempts = 0;
    mockSettings(settings("stopped"), {
      settings_get: () => {
        attempts += 1;
        if (attempts === 1) {
          throw { code: "internal_error", messageId: "" };
        }
        return settings("stopped");
      },
    });
    renderPage();
    expect(await screen.findByText("Unable to load settings. Please try again.")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("Stopped")).toBeInTheDocument();
  });
});
