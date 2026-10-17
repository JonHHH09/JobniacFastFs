// One WebDriver session of the desktop app through tauri-driver, plus failure screenshots.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { remote } from "webdriverio";
import { appBinary, artifactsDir } from "./config.mjs";

const DRIVER_HOST = "127.0.0.1";
const DRIVER_PORT = 4444;
const exeSuffix = process.platform === "win32" ? ".exe" : "";

/** tauri-driver from `cargo install tauri-driver --locked`, or the one named by TAURI_DRIVER. */
function tauriDriverCommand() {
  if (process.env.TAURI_DRIVER) {
    return process.env.TAURI_DRIVER;
  }
  const cargoBin = join(homedir(), ".cargo", "bin", `tauri-driver${exeSuffix}`);
  return existsSync(cargoBin) ? cargoBin : "tauri-driver";
}

/**
 * Starts tauri-driver with `env` (the application inherits it) and opens one session on the
 * application binary. The returned object is closed with `closeSession`.
 */
export async function openSession(env) {
  const stderr = [];
  // Its own process group on POSIX, so closing the session ends the application as well.
  const driver = spawn(tauriDriverCommand(), [], {
    env,
    stdio: ["ignore", "ignore", "pipe"],
    windowsHide: true,
    detached: process.platform !== "win32",
  });
  driver.stderr.on("data", (chunk) => {
    stderr.push(String(chunk));
  });
  driver.on("error", (error) => {
    stderr.push(`tauri-driver could not start: ${error.message}`);
  });
  const session = { driver, browser: undefined, driverLog: stderr };
  try {
    session.browser = await remote({
      hostname: DRIVER_HOST,
      port: DRIVER_PORT,
      path: "/",
      logLevel: "error",
      connectionRetryTimeout: 120_000,
      connectionRetryCount: 3,
      capabilities: {
        browserName: "wry",
        // WebdriverIO otherwise requests a BiDi session (webSocketUrl), which WebKitWebDriver on
        // Linux rejects with "Failed to match capabilities". The specs only use classic WebDriver.
        "wdio:enforceWebDriverClassic": true,
        "tauri:options": { application: appBinary },
      },
    });
    return session;
  } catch (error) {
    await closeSession(session);
    throw error;
  }
}

/** Ends the WebDriver session (if the window has not already closed) and stops tauri-driver. */
export async function closeSession(session) {
  try {
    await session.browser?.deleteSession();
  } catch {
    // The window may already have closed (the close-gate test ends the application).
  }
  stopProcessTree(session.driver);
}

/**
 * Ends tauri-driver with the application and msedgedriver it started. A window that closed or a
 * failed session can leave the application running, so the whole tree is ended, not only the driver.
 */
function stopProcessTree(child) {
  if (child.pid === undefined || child.exitCode !== null) {
    return;
  }
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true });
  } else {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill();
    }
  }
}

/** Saves a screenshot of a failed test into the ignored artifacts folder; returns its path. */
export async function captureFailure(browser, name) {
  mkdirSync(artifactsDir, { recursive: true });
  const safe = name.replace(/[^A-Za-z0-9_-]+/g, "_").slice(0, 120);
  const path = join(artifactsDir, `${safe}-${Date.now()}.png`);
  try {
    await browser.saveScreenshot(path);
    return path;
  } catch {
    return undefined;
  }
}
