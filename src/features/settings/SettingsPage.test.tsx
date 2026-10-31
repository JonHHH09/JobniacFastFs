import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type {
  FeatureDto,
  FeatureIdDto,
  FeaturesDto,
  McpServerStatusDto,
  SettingsDto,
} from "../../ipc/bindings";
import { SettingsPage } from "./SettingsPage";
import { RUNNING_POLL_MS } from "./useMcpServer";

function settings(status: McpServerStatusDto, port = 8765, enabled = status !== "stopped") {
  return { mcp: { enabled, port, status, stopIncomplete: false } } satisfies SettingsDto;
}

const featureIds: FeatureIdDto[] = [
  "arbeitnow",
  "linkFetch",
  "deletion",
  "documentArtifacts",
  "generation",
  "mcpArbeitnow",
  "mcpLinkFetch",
  "mcpDeletion",
];

/** The capabilities of the MCP server that reach the network or delete data, off until opt-in. */
const mcpCapabilityIds: FeatureIdDto[] = ["mcpArbeitnow", "mcpLinkFetch", "mcpDeletion"];

/**
 * The first-run state: every in-app feature on and running, the MCP capabilities off and not
 * running, unless `over` changes it.
 */
function features(
  over: Partial<Record<FeatureIdDto, Partial<FeatureDto>>> = {},
  stopIncomplete = false,
): FeaturesDto {
  return {
    stopIncomplete,
    features: featureIds.map((id) => {
      const mcp = mcpCapabilityIds.includes(id);
      return {
        id,
        enabled: !mcp,
        effective: !mcp,
        forcedByEnvironment: false,
        pendingRestart: false,
        ...over[id],
      };
    }),
  };
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <SettingsPage />
    </QueryClientProvider>,
  );
}

/** Mocks the commands of the page; `extra` handles or overrides any of them. */
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
    if (cmd === "features_get") {
      return features();
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

describe("optional features", () => {
  const label = (text: string) => screen.findByRole("switch", { name: text });

  it("lists the in-app features on by default and a restart note", async () => {
    mockSettings(settings("stopped"));
    renderPage();
    const card = await screen.findByTestId("featuresCard");
    expect(card).toHaveTextContent("Optional features");
    for (const name of [
      "Arbeitnow job board",
      "Add a job from a link",
      "Delete profiles and jobs",
      "Document preview and export",
      "Generate resume and cover-letter PDFs",
    ]) {
      expect(await label(name)).toBeChecked();
      expect(await label(name)).toBeEnabled();
    }
    expect(card).toHaveTextContent(
      "Changes apply after you restart the app, except turning off an MCP capability.",
    );
  });

  it("shows the three MCP capabilities off by default, apart from the in-app features", async () => {
    mockSettings(settings("stopped"));
    renderPage();
    const group = await screen.findByTestId("featuresMcpGroup");
    expect(group).toHaveTextContent("MCP clients: network and deleting");
    expect(group).toHaveTextContent("These are off by default.");
    expect(group).toHaveTextContent("do not ask for the confirmation the app asks for");
    // A general-purpose environment variable is no opt-in, and the screen says so.
    expect(group).toHaveTextContent("JOBNIAC_DELETION_ENABLED=1 does not turn them on");
    // Turning one off is immediate and restarts the server; turning one on needs an app restart.
    expect(group).toHaveTextContent("Turning one of these off takes effect at once");
    expect(group).toHaveTextContent("MCP clients must reconnect with the new access token");
    expect(group).toHaveTextContent("Turning one on applies after you restart the app.");
    for (const name of [
      "Let MCP clients scan Arbeitnow",
      "Let MCP clients add a job from a link",
      "Let MCP clients delete profiles and jobs",
    ]) {
      const toggle = await label(name);
      expect(group).toContainElement(toggle);
      expect(toggle).not.toBeChecked();
      expect(toggle).toBeEnabled();
    }
    // The in-app switches stay in the first list, on.
    expect(group).not.toContainElement(await label("Delete profiles and jobs"));
    expect(await label("Delete profiles and jobs")).toBeChecked();
    expect(screen.getByTestId("feature-mcpDeletion")).toHaveTextContent(
      "delete_profile and delete_job",
    );
    expect(screen.getByTestId("feature-mcpLinkFetch")).toHaveTextContent("add_job_from_link");
    expect(screen.getByTestId("feature-mcpArbeitnow")).toHaveTextContent(
      "scan_arbeitnow_jobs and import_arbeitnow_job",
    );
    expect(screen.getByTestId("feature-mcpArbeitnow")).toHaveTextContent(
      "contacts the Arbeitnow job board",
    );
    // The in-app Arbeitnow row keeps its own label and stays on.
    expect(await label("Arbeitnow job board")).toBeChecked();
    // Off and not started: nothing waits for a restart and nothing failed to start.
    expect(screen.getByTestId("feature-mcpDeletion")).not.toHaveTextContent("Applies after");
    expect(screen.getByTestId("feature-mcpDeletion")).not.toHaveTextContent("Could not start");
  });

  it("opts an MCP capability in only when its own switch is turned on", async () => {
    const invoke = mockSettings(settings("stopped"), {
      feature_set_enabled: (args) =>
        features({
          [args.id as FeatureIdDto]: { enabled: args.enabled === true, pendingRestart: true },
        }),
    });
    renderPage();
    await userEvent.click(await label("Let MCP clients delete profiles and jobs"));

    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    expect(calls(invoke, "feature_set_enabled")[0]?.[1]).toEqual({
      id: "mcpDeletion",
      enabled: true,
    });
    await waitFor(async () =>
      expect(await label("Let MCP clients delete profiles and jobs")).toBeChecked(),
    );
    expect(screen.getByTestId("feature-mcpDeletion")).toHaveTextContent(
      "Applies after you restart the app.",
    );
    // The other capabilities and the in-app switches are untouched.
    expect(await label("Let MCP clients add a job from a link")).not.toBeChecked();
    expect(await label("Let MCP clients scan Arbeitnow")).not.toBeChecked();
    expect(await label("Delete profiles and jobs")).toBeChecked();
    expect(calls(invoke, "feature_set_enabled")).toHaveLength(1);
  });

  it("says a server that could not be confirmed stopped may still be running", async () => {
    mockSettings(
      { mcp: { enabled: false, port: 8765, status: "failed", stopIncomplete: true } },
      { features_get: () => features() },
    );
    renderPage();
    const card = await screen.findByTestId("mcpServerCard");
    expect(card).toHaveTextContent("could not be confirmed stopped");
    expect(card).toHaveTextContent("Close and reopen the app to be sure.");
    // Not the message of a failed start.
    expect(card).not.toHaveTextContent("The server did not start.");
  });

  it("does not show a revoked capability as safely off while the server may still serve it", async () => {
    mockSettings(
      { mcp: { enabled: true, port: 8765, status: "failed", stopIncomplete: true } },
      {
        features_get: () => features({ mcpDeletion: { enabled: false, effective: false } }, true),
      },
    );
    renderPage();
    const group = await screen.findByTestId("featuresMcpGroup");
    // The existing stop-incomplete warning is shown with the capabilities, not only in the server card.
    const warning = await screen.findByTestId("featuresStopIncomplete");
    expect(group).toContainElement(warning);
    expect(warning).toHaveTextContent("could not be confirmed stopped");
    expect(warning).toHaveTextContent("Close and reopen the app to be sure.");
    // The revoked row says it may still be served; a capability that is on does not need to.
    expect(screen.getByTestId("feature-mcpDeletion")).toHaveTextContent(
      "this tool may still be served until it is gone",
    );
    expect(screen.getByTestId("feature-mcpLinkFetch")).toHaveTextContent(
      "this tool may still be served until it is gone",
    );
    // The in-app rows are not part of it.
    expect(screen.getByTestId("feature-deletion")).not.toHaveTextContent("may still be served");
    expect(screen.getByTestId("feature-arbeitnow")).not.toHaveTextContent("may still be served");
    expect(await label("Let MCP clients delete profiles and jobs")).not.toBeChecked();
  });

  it("shows no such warning or note when the server was stopped", async () => {
    mockSettings(settings("running"));
    renderPage();
    await screen.findByTestId("featuresMcpGroup");
    expect(screen.queryByTestId("featuresStopIncomplete")).not.toBeInTheDocument();
    for (const id of ["mcpArbeitnow", "mcpLinkFetch", "mcpDeletion"]) {
      expect(screen.getByTestId(`feature-${id}`)).not.toHaveTextContent("may still be served");
    }
  });

  it("reads both screens again while a stop is incomplete and drops the warning once it is gone", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let healed = false;
      mockSettings(settings("running"), {
        settings_get: () =>
          healed
            ? settings("stopped")
            : { mcp: { enabled: true, port: 8765, status: "failed", stopIncomplete: true } },
        features_get: () =>
          features({ mcpDeletion: { enabled: false, effective: false } }, !healed),
      });
      renderPage();
      expect(await screen.findByTestId("featuresStopIncomplete")).toBeVisible();
      healed = true;
      await vi.advanceTimersByTimeAsync(RUNNING_POLL_MS + 100);
      await waitFor(() =>
        expect(screen.queryByTestId("featuresStopIncomplete")).not.toBeInTheDocument(),
      );
      expect(screen.getByTestId("feature-mcpDeletion")).not.toHaveTextContent(
        "may still be served",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the failed-start hint for a start that failed", async () => {
    mockSettings(settings("failed"));
    renderPage();
    const card = await screen.findByTestId("mcpServerCard");
    expect(card).toHaveTextContent("The server did not start.");
    expect(card).not.toHaveTextContent("could not be confirmed stopped");
  });

  it("shows the fixed message when a revocation could not stop the server and reads both screens again", async () => {
    let featureReads = 0;
    let settingsReads = 0;
    let revoked = false;
    const invoke = mockSettings(settings("running"), {
      settings_get: () => {
        settingsReads += 1;
        return revoked
          ? { mcp: { enabled: true, port: 8765, status: "failed", stopIncomplete: true } }
          : settings("running");
      },
      features_get: () => {
        featureReads += 1;
        // After the failed command the switch is saved off and the grant is gone.
        return revoked
          ? features({
              mcpDeletion: { enabled: false, effective: false, pendingRestart: false },
            })
          : features({ mcpDeletion: { enabled: true, effective: true } });
      },
      feature_set_enabled: () => {
        revoked = true;
        throw { code: "internal_error", messageId: "settings.mcp.stopIncomplete" };
      },
    });
    renderPage();
    const toggle = await label("Let MCP clients delete profiles and jobs");
    expect(toggle).toBeChecked();
    await screen.findByTestId("mcpStatus");
    const featureReadsBefore = featureReads;
    const settingsReadsBefore = settingsReads;
    await userEvent.click(toggle);
    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    // The fixed message, with no raw error text.
    expect(
      (
        await screen.findAllByText(
          /could not be confirmed stopped, so a copy of it may still be running/,
        )
      ).length,
    ).toBeGreaterThan(0);
    // The error does not leave a stale screen: the switch and the server status are read again.
    await waitFor(() => expect(featureReads).toBeGreaterThan(featureReadsBefore));
    await waitFor(() => expect(settingsReads).toBeGreaterThan(settingsReadsBefore));
    await waitFor(async () =>
      expect(await label("Let MCP clients delete profiles and jobs")).not.toBeChecked(),
    );
    const card = await screen.findByTestId("mcpServerCard");
    await waitFor(() => expect(card).toHaveTextContent("Close and reopen the app to be sure."));
  });

  it("shows a revoked MCP capability off at once and reads the server status again", async () => {
    let revoked = false;
    let settingsReads = 0;
    const invoke = mockSettings(settings("running"), {
      settings_get: () => {
        settingsReads += 1;
        return settings("running");
      },
      features_get: () =>
        features({ mcpDeletion: { enabled: true, effective: true, pendingRestart: false } }),
      feature_set_enabled: (args) => {
        revoked = args.enabled === false;
        // Rust has restarted the server without the tool: off, not usable, nothing waits.
        return features({
          [args.id as FeatureIdDto]: { enabled: false, effective: false, pendingRestart: false },
        });
      },
    });
    renderPage();
    const toggle = await label("Let MCP clients delete profiles and jobs");
    expect(toggle).toBeChecked();
    await screen.findByTestId("mcpStatus");
    const readsBefore = settingsReads;
    await userEvent.click(toggle);
    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    expect(revoked).toBe(true);
    expect(calls(invoke, "feature_set_enabled")[0]?.[1]).toEqual({
      id: "mcpDeletion",
      enabled: false,
    });
    await waitFor(async () =>
      expect(await label("Let MCP clients delete profiles and jobs")).not.toBeChecked(),
    );
    const row = screen.getByTestId("feature-mcpDeletion");
    expect(row).not.toHaveTextContent("Applies after");
    expect(row).not.toHaveTextContent("Could not start");
    // The server was restarted by the command, so its status and token are read again.
    await waitFor(() => expect(settingsReads).toBeGreaterThan(readsBefore));
  });

  it("says a revoked capability turned on again waits for a restart of the app", async () => {
    mockSettings(settings("running"), {
      features_get: () =>
        features({ mcpLinkFetch: { enabled: true, effective: false, pendingRestart: true } }),
    });
    renderPage();
    const toggle = await label("Let MCP clients add a job from a link");
    expect(toggle).toBeChecked();
    const row = screen.getByTestId("feature-mcpLinkFetch");
    expect(row).toHaveTextContent("Applies after you restart the app.");
    expect(row).not.toHaveTextContent("Could not start");
  });

  it("opts the Arbeitnow scan in with its own switch, not the in-app Arbeitnow one", async () => {
    const invoke = mockSettings(settings("stopped"), {
      feature_set_enabled: (args) =>
        features({
          [args.id as FeatureIdDto]: { enabled: args.enabled === true, pendingRestart: true },
        }),
    });
    renderPage();
    await userEvent.click(await label("Let MCP clients scan Arbeitnow"));
    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    expect(calls(invoke, "feature_set_enabled")[0]?.[1]).toEqual({
      id: "mcpArbeitnow",
      enabled: true,
    });
    expect(await label("Arbeitnow job board")).toBeChecked();
  });

  it("turning an in-app switch off or on never sends an MCP opt-in", async () => {
    const invoke = mockSettings(settings("stopped"), {
      feature_set_enabled: (args) =>
        features({ [args.id as FeatureIdDto]: { enabled: args.enabled === true } }),
    });
    renderPage();
    await userEvent.click(await label("Delete profiles and jobs"));
    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    expect(calls(invoke, "feature_set_enabled")[0]?.[1]).toEqual({
      id: "deletion",
      enabled: false,
    });
    expect(await label("Let MCP clients delete profiles and jobs")).not.toBeChecked();
    expect(await label("Let MCP clients add a job from a link")).not.toBeChecked();
    expect(await label("Let MCP clients scan Arbeitnow")).not.toBeChecked();
  });

  it("shows an opted-in MCP capability on and lets it be turned off again", async () => {
    const invoke = mockSettings(settings("stopped"), {
      features_get: () => features({ mcpLinkFetch: { enabled: true, effective: true } }),
      feature_set_enabled: (args) =>
        features({
          [args.id as FeatureIdDto]: { enabled: args.enabled === true, pendingRestart: true },
        }),
    });
    renderPage();
    const toggle = await label("Let MCP clients add a job from a link");
    expect(toggle).toBeChecked();
    await userEvent.click(toggle);
    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    expect(calls(invoke, "feature_set_enabled")[0]?.[1]).toEqual({
      id: "mcpLinkFetch",
      enabled: false,
    });
  });

  it("disables an MCP switch that an environment variable refuses and shows it off", async () => {
    // A variable set to something other than `1` refuses the capability over an opt-in.
    mockSettings(settings("stopped"), {
      features_get: () =>
        features({
          mcpDeletion: { enabled: true, effective: false, forcedByEnvironment: true },
        }),
    });
    renderPage();
    const toggle = await label("Let MCP clients delete profiles and jobs");
    expect(toggle).toBeDisabled();
    expect(toggle).not.toBeChecked();
    expect(screen.getByTestId("feature-mcpDeletion")).toHaveTextContent(
      "An environment variable sets this one, so the switch has no effect.",
    );
    expect(screen.getByTestId("feature-mcpDeletion")).not.toHaveTextContent("Could not start");
  });

  it("leaves the MCP switches in the user's hands when an environment variable enables the in-app feature", async () => {
    // `JOBNIAC_*_ENABLED=1` forces the in-app feature on; Rust reports the MCP rows as not forced.
    mockSettings(settings("stopped"), {
      features_get: () =>
        features({
          deletion: { forcedByEnvironment: true, effective: true },
          linkFetch: { forcedByEnvironment: true, effective: true },
          arbeitnow: { forcedByEnvironment: true, effective: true },
        }),
    });
    renderPage();
    for (const name of [
      "Delete profiles and jobs",
      "Add a job from a link",
      "Arbeitnow job board",
    ]) {
      const inApp = await label(name);
      expect(inApp).toBeDisabled();
      expect(inApp).toBeChecked();
    }
    for (const name of [
      "Let MCP clients delete profiles and jobs",
      "Let MCP clients add a job from a link",
      "Let MCP clients scan Arbeitnow",
    ]) {
      const mcp = await label(name);
      expect(mcp).toBeEnabled();
      expect(mcp).not.toBeChecked();
    }
  });

  it("saves one switch and shows the answer, noting that it applies after a restart", async () => {
    const invoke = mockSettings(settings("stopped"), {
      feature_set_enabled: (args) =>
        features({
          [args.id as FeatureIdDto]: { enabled: args.enabled === true, pendingRestart: true },
        }),
    });
    renderPage();
    await userEvent.click(await label("Delete profiles and jobs"));

    await waitFor(() => expect(calls(invoke, "feature_set_enabled")).toHaveLength(1));
    expect(calls(invoke, "feature_set_enabled")[0]?.[1]).toEqual({
      id: "deletion",
      enabled: false,
    });
    await waitFor(async () => expect(await label("Delete profiles and jobs")).not.toBeChecked());
    expect(screen.getByTestId("feature-deletion")).toHaveTextContent(
      "Applies after you restart the app.",
    );
    expect(await label("Arbeitnow job board")).toBeChecked();
  });

  it("disables a switch that an environment variable forces and says why", async () => {
    const forced = features({
      generation: {
        enabled: true,
        effective: false,
        forcedByEnvironment: true,
        pendingRestart: false,
      },
    });
    mockSettings(settings("stopped"), { features_get: () => forced });
    renderPage();
    const toggle = await label("Generate resume and cover-letter PDFs");
    expect(toggle).toBeDisabled();
    expect(screen.getByTestId("feature-generation")).toHaveTextContent(
      "An environment variable sets this one, so the switch has no effect.",
    );
    // A forced feature shows no restart note even though it differs from the switch.
    expect(screen.getByTestId("feature-generation")).not.toHaveTextContent("Applies after");
    expect(await label("Arbeitnow job board")).toBeEnabled();
  });

  it("shows a forced switch as what runs, not as the saved value", async () => {
    mockSettings(settings("stopped"), {
      features_get: () =>
        features({
          deletion: { enabled: true, effective: false, forcedByEnvironment: true },
          linkFetch: { enabled: false, effective: true, forcedByEnvironment: true },
        }),
    });
    renderPage();
    const off = await label("Delete profiles and jobs");
    expect(off).toBeDisabled();
    expect(off).not.toBeChecked();
    const on = await label("Add a job from a link");
    expect(on).toBeDisabled();
    expect(on).toBeChecked();
  });

  it("says a feature could not start only when it is on, not running and nothing is pending", async () => {
    mockSettings(settings("stopped"), {
      features_get: () =>
        features({
          arbeitnow: { effective: false },
          deletion: { effective: false, pendingRestart: true },
          generation: { effective: false, forcedByEnvironment: true },
        }),
    });
    renderPage();
    await label("Arbeitnow job board");
    expect(screen.getByTestId("feature-arbeitnow")).toHaveTextContent(
      "Could not start; see the log.",
    );
    expect(screen.getByTestId("feature-arbeitnow")).not.toHaveTextContent("Applies after");
    expect(screen.getByTestId("feature-deletion")).toHaveTextContent("Applies after you restart");
    expect(screen.getByTestId("feature-deletion")).not.toHaveTextContent("Could not start");
    expect(screen.getByTestId("feature-generation")).not.toHaveTextContent("Could not start");
    expect(screen.getByTestId("feature-linkFetch")).not.toHaveTextContent("Could not start");
  });

  it("shows the save failure copy and keeps the saved value", async () => {
    mockSettings(settings("stopped"), {
      feature_set_enabled: () => {
        throw { code: "internal_error", messageId: "settings.saveFailed" };
      },
    });
    renderPage();
    await userEvent.click(await label("Arbeitnow job board"));
    expect(await screen.findByText("Unable to save settings. Please try again.")).toBeVisible();
    expect(await label("Arbeitnow job board")).toBeChecked();
  });

  it("shows a load error with a retry", async () => {
    let attempts = 0;
    mockSettings(settings("stopped"), {
      features_get: () => {
        attempts += 1;
        if (attempts === 1) {
          throw { code: "internal_error", messageId: "" };
        }
        return features();
      },
    });
    renderPage();
    expect(
      await screen.findByText("Unable to load the optional features. Please try again."),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await label("Arbeitnow job board")).toBeChecked();
  });
});
