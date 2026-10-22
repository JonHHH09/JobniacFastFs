import process from "node:process";
import { defineConfig, devices } from "@playwright/test";

// Browser UI suite: the real React app in Chromium against the in-memory mock IPC (src/dev), so it
// checks rendering, navigation and UI state without the Rust core. The Rust core, the database and
// the native webview stay covered by the WebDriver suite in e2e/.
const PORT = 1431;

export default defineConfig({
  testDir: "./playwright",
  outputDir: "./playwright/.artifacts",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    launchOptions: {
      // PLAYWRIGHT_SLOWMO=300 pauses 300 ms between actions, for watching a headed run.
      slowMo: Number(process.env.PLAYWRIGHT_SLOWMO) || 0,
      // Use an installed Chromium instead of the downloaded one, for example /usr/bin/chromium.
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_PATH || undefined,
    },
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `pnpm dev:mock --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
