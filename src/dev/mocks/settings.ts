import type { McpServerStatusDto, SettingsDto } from "../../ipc/bindings";
import { type Args, fail, type HandlerMap } from "./util";

// Synthetic, stateful stand-in for the settings commands. Turning the server on shows `starting`
// for a moment, then `running`. Port 9999 simulates a port in use (the server then fails to
// start) and port 9998 a missing sidecar (`unavailable`).
const START_DELAY_MS = 1500;
const PORT_IN_USE = 9999;
const NO_SIDECAR = 9998;

const state = { enabled: false, port: 8765, status: "stopped" as McpServerStatusDto };

function snapshot(): SettingsDto {
  return { mcp: { enabled: state.enabled, port: state.port, status: state.status } };
}

function start(): void {
  if (state.port === NO_SIDECAR) {
    state.status = "unavailable";
    return;
  }
  state.status = "starting";
  const port = state.port;
  setTimeout(() => {
    if (state.enabled && state.status === "starting" && state.port === port) {
      state.status = port === PORT_IN_USE ? "failed" : "running";
    }
  }, START_DELAY_MS);
}

export const settingsHandlers: HandlerMap = {
  settings_get: (): SettingsDto => snapshot(),
  mcp_server_set_enabled: (args: Args): SettingsDto => {
    state.enabled = args.enabled === true;
    if (state.enabled) {
      start();
    } else {
      state.status = "stopped";
    }
    return snapshot();
  },
  mcp_server_set_port: (args: Args): SettingsDto => {
    const port = Number(args.port);
    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      return fail("validation_error", "settings.mcp.invalidPort");
    }
    state.port = port;
    if (state.enabled) {
      start();
    }
    return snapshot();
  },
  mcp_server_copy_connection: (): null => {
    if (state.status !== "running") {
      return fail("not_configured", "settings.mcp.notRunning");
    }
    return null;
  },
};
