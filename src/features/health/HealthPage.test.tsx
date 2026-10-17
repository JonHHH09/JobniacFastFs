import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { HealthReportDto } from "../../ipc/bindings";
import { formatInstant } from "../../lib/format";
import { HealthPage } from "./HealthPage";

// Synthetic report: no real host, database or user data.
const upReport: HealthReportDto = {
  status: "UP",
  errorCategory: "NONE",
  metadata: {
    checkedAt: "2026-01-02T03:04:05.123456Z",
    totalChecks: 1,
    successfulChecks: 1,
    failedChecks: 0,
  },
  generatedResumeCleanup: {
    status: "UNKNOWN",
    pendingCount: "0",
    processingCount: "0",
    expiredCompletedCount: "0",
    oldestDueAgeSeconds: "0",
    repeatedFailure: false,
  },
};

const downReport: HealthReportDto = {
  ...upReport,
  status: "DOWN",
  errorCategory: "CONNECTION_UNAVAILABLE",
  metadata: { ...upReport.metadata, successfulChecks: 0, failedChecks: 1 },
};

// The report the shared health use case returns for an adapter-level failure:
// DOWN, UNKNOWN, one failed check and a check time.
const unknownReport: HealthReportDto = {
  ...downReport,
  errorCategory: "UNKNOWN",
  metadata: { ...downReport.metadata, checkedAt: "2026-03-04T05:06:07.000000Z" },
};

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <HealthPage />
    </QueryClientProvider>,
  );
}

/** The one live region that announces the outcome. */
function statusRegion() {
  return screen.getByRole("status");
}

describe("HealthPage", () => {
  it("does not run the check at startup and shows the not-checked state", () => {
    const invoke = vi.fn(() => upReport);
    mockIPC(invoke);

    renderPage();

    expect(statusRegion()).toHaveTextContent("Health status: Not checked");
    expect(screen.getByRole("button", { name: "Refresh health" })).toBeEnabled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("runs the check on request and shows the report", async () => {
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        return upReport;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    expect(await screen.findByText("Health status: Up")).toBeInTheDocument();
    expect(screen.getByText(formatInstant("2026-01-02T03:04:05.123456Z"))).toBeInTheDocument();
    expect(screen.getByText("Successful checks").nextElementSibling).toHaveTextContent("1");
    expect(screen.getByText("Failed checks").nextElementSibling).toHaveTextContent("0");
    expect(screen.getByText("Cleanup status is not available yet.")).toBeInTheDocument();
  });

  it("names the error category when the check reports DOWN", async () => {
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        return downReport;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    expect(await screen.findByText("Health status: Down")).toBeInTheDocument();
    expect(screen.getByText("The health check failed: Connection unavailable")).toBeInTheDocument();
    expect(screen.getByText("Failed checks").nextElementSibling).toHaveTextContent("1");
  });

  it("renders the failure report from the shared check with one failed check and a check time", async () => {
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        return unknownReport;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    expect(await screen.findByText("The health check failed: Unknown")).toBeInTheDocument();
    expect(screen.getByText("Health status: Down")).toBeInTheDocument();
    expect(screen.getByText("Failed checks").nextElementSibling).toHaveTextContent("1");
    expect(screen.getByText("Successful checks").nextElementSibling).toHaveTextContent("0");
    expect(screen.getByText(formatInstant("2026-03-04T05:06:07.000000Z"))).toBeInTheDocument();
    expect(screen.getByText("Cleanup status is not available yet.")).toBeInTheDocument();
  });

  it("shows the unknown failure report for a typed startup failure, with no raw value", async () => {
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        // The Rust command rejects with a CommandError object.
        throw { code: "internal_error", messageId: "startup.failed" };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Refresh health" })).toBeEnabled();
    });
    expect(statusRegion()).toHaveTextContent("Health status: Down");
    expect(screen.getByText("The health check failed: Unknown")).toBeInTheDocument();
    expect(screen.queryByText(/could not start/)).not.toBeInTheDocument();
  });

  it("does not show a raw string rejection, and shows the unknown failure report", async () => {
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        throw "raw secret text from the runtime";
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Refresh health" })).toBeEnabled();
    });
    expect(statusRegion()).toHaveTextContent("Health status: Down");
    expect(screen.queryByText(/raw secret/)).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("raw secret text from the runtime");
  });

  it("shows the failure report over a stale report when a later check fails", async () => {
    let calls = 0;
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        calls += 1;
        if (calls === 2) {
          throw { code: "internal_error", messageId: "startup.failed" };
        }
        return upReport;
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));
    expect(await screen.findByText("Health status: Up")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    expect(await screen.findByText("Health status: Down")).toBeInTheDocument();
    expect(screen.getByText("The health check failed: Unknown")).toBeInTheDocument();
  });

  it("does not show the busy refusal as copy", async () => {
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        throw { code: "busy", messageId: "health.busy" };
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Refresh health" })).toBeEnabled();
    });
    expect(statusRegion()).toHaveTextContent("Health status: Not checked");
    expect(screen.queryByText(/already running/)).not.toBeInTheDocument();
  });

  it("starts one check for two rapid activations", async () => {
    let calls = 0;
    let finish: (value: unknown) => void = () => undefined;
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        calls += 1;
        return new Promise((resolve) => {
          finish = resolve;
        });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    const button = screen.getByRole("button", { name: "Refresh health" });
    fireEvent.click(button);
    fireEvent.click(button);

    await waitFor(() => {
      expect(calls).toBe(1);
    });
    finish(upReport);
    expect(await screen.findByText("Health status: Up")).toBeInTheDocument();
    expect(calls).toBe(1);
  });

  it("shows the checking state and disables the button while the check runs", async () => {
    let finish: (value: unknown) => void = () => undefined;
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        return new Promise((resolve) => {
          finish = resolve;
        });
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));

    expect(
      await screen.findByText(
        "The check runs outside the interface so this window stays responsive.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Checking…" })).toBeDisabled();

    finish(upReport);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Refresh health" })).toBeEnabled();
    });
  });

  it("shows the degraded and healthy cleanup statuses as their names", async () => {
    const reports = [
      {
        ...upReport,
        generatedResumeCleanup: { ...upReport.generatedResumeCleanup, status: "DEGRADED" as const },
      },
      {
        ...upReport,
        generatedResumeCleanup: { ...upReport.generatedResumeCleanup, status: "HEALTHY" as const },
      },
    ];
    let next = 0;
    mockIPC((cmd) => {
      if (cmd === "health_check") {
        return reports[next++];
      }
      throw new Error(`unexpected command ${cmd}`);
    });

    renderPage();
    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));
    expect(await screen.findByText("Cleanup status: Degraded")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Refresh health" }));
    expect(await screen.findByText("Cleanup status: Healthy")).toBeInTheDocument();
  });

  it("shows the replacement footer at the end of the screen", () => {
    mockIPC(() => upReport);

    renderPage();

    expect(screen.getByText("This screen shares the health check with MCP.")).toBeInTheDocument();
  });
});
