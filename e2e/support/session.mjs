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
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
    detached: process.platform !== "win32",
  });
  // tauri-driver and the native driver it starts (msedgedriver, WebKitWebDriver) write to both streams.
  for (const stream of [driver.stdout, driver.stderr]) {
    stream.on("data", (chunk) => {
      stderr.push(String(chunk));
    });
  }
  driver.on("exit", (code, signal) => {
    stderr.push(`tauri-driver exited with code ${code} signal ${signal}\n`);
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
      // One attempt: the native driver gives up by itself on a window that never opens, and a
      // retry would only repeat the wait past the mocha timeout, which hides the cause.
      connectionRetryTimeout: 120_000,
      connectionRetryCount: 0,
      capabilities: {
        browserName: "wry",
        // `webviewOptions` (tauri-driver 2.1 passes it to msedgedriver on Windows, Linux ignores it)
        // marks the binary as a WebView2 application. Without it msedgedriver starts the binary as
        // a browser, waits for a DevToolsActivePort file the application never writes and fails
        // with "session not created: DevToolsActivePort file doesn't exist".
        "tauri:options": { application: appBinary, webviewOptions: {} },
        // WebdriverIO 10 asks for a BiDi session (`webSocketUrl`), which WebKitWebDriver and
        // msedgedriver behind tauri-driver do not support; stay on the classic protocol.
        "wdio:enforceWebDriverClassic": true,
      },
    });
    return session;
  } catch (error) {
    // A session that never opened leaves nothing else to read: print what the drivers said and
    // which application processes are still alive, before they are ended.
    console.error(`session not opened: ${error.message}`);
    console.error(`tauri-driver: ${stderr.join("").slice(-4000)}`);
    console.error(`processes: ${processList()}`);
    await closeSession(session);
    throw error;
  }
}

/** The application and driver processes that are running, for the failure report. */
function processList() {
  const result =
    process.platform === "win32"
      ? spawnSync("tasklist", ["/FO", "CSV", "/NH"], { encoding: "utf8", windowsHide: true })
      : spawnSync("ps", ["-eo", "pid,etime,args"], { encoding: "utf8" });
  const lines = String(result.stdout ?? "").split("\n");
  return lines.filter((line) => /jobniac|msedge|webkit|tauri-driver/i.test(line)).join("\n");
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
