// Native smoke of the debug desktop build, isolated from the per-user folders of the machine:
//
//   node e2e/native-smoke.mjs [--prove-only]
//
// `--prove-only` runs the pre-launch proof and stops: nothing is started.
//
// It launches the real `jobniac` binary once with the packaging smoke quit hook
// (JOBNIAC_SMOKE_QUIT_AFTER_LOAD=1): the window opens, the page loads, the health use case answers
// and the process exits 0. No WebDriver and no account is involved. The run root, the database,
// the generated root, the temporary directory and every Tauri app directory (settings, keys, log,
// the WebView2 profile) are inside `target/e2e-*`; the isolation is proven BEFORE the application
// starts (the runner never launches when the proof fails) and the real folders are shown unchanged
// afterwards. See e2e/README.md ("Native smoke") for what this does and does not cover.
import { spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import { join } from "node:path";
import {
  appBinary,
  cliBinary,
  createRunRoot,
  repoRoot,
  sessionEnvironment,
} from "./support/config.mjs";
import { runCli } from "./support/database.mjs";
import {
  observeRunRoot,
  proveBeforeLaunch,
  snapshotRealFolders,
  verifyAfterRun,
} from "./support/isolation.mjs";

const LAUNCH_TIMEOUT_MS = 90_000;

function fail(message) {
  console.error(`native-smoke: ${message}`);
  process.exitCode = 2;
}

/** Runs the application once; resolves with its exit status and the tail of its output. */
function launch(env) {
  return new Promise((resolve) => {
    const child = spawn(appBinary, [], { env, stdio: ["ignore", "pipe", "pipe"] });
    const output = [];
    for (const stream of [child.stdout, child.stderr]) {
      stream.on("data", (chunk) => output.push(String(chunk)));
    }
    const timer = setTimeout(() => {
      child.kill();
      output.push("native-smoke: the launch timeout ended the application\n");
    }, LAUNCH_TIMEOUT_MS);
    child.on("exit", (code, signal) => {
      clearTimeout(timer);
      resolve({ code, signal, output: output.join("").slice(-2000) });
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ code: null, signal: null, output: `could not start: ${error.message}` });
    });
  });
}

async function main() {
  if (!existsSync(appBinary)) {
    return fail(
      `the desktop binary is missing at ${appBinary}; run cargo build -p jobniac-desktop`,
    );
  }
  if (!existsSync(cliBinary)) {
    return fail(`the console binary is missing at ${cliBinary}; run cargo build -p jobniac-cli`);
  }
  const run = createRunRoot();
  try {
    const pdfiumDirectory = join(repoRoot, "vendor", "pdfium", "windows-x64");
    const environment = sessionEnvironment(process.env, {
      JOBNIAC_DB_PATH: run.databaseFile,
      ...run.userDirectories,
      ...run.isolation,
      JOBNIAC_DOCUMENT_IMPORT_ROOT: run.importRoot,
      JOBNIAC_GENERATED_ROOT: run.generatedRoot,
      JOBNIAC_SMOKE_QUIT_AFTER_LOAD: "1",
      // Also exercises the redirected log directory.
      JOBNIAC_LOG_FILE: "1",
      ...(process.platform === "win32" && existsSync(pdfiumDirectory)
        ? { JOBNIAC_PDFIUM_DIR: pdfiumDirectory }
        : {}),
    });

    // 1. The proof. A failure here ends the script: the application is not started.
    const realEnvironment = { ...process.env };
    for (const fact of proveBeforeLaunch({
      run,
      appBinaryPath: appBinary,
      env: environment,
      realEnvironment,
    })) {
      console.log(`native-smoke: before launch: ${fact}`);
    }
    const before = snapshotRealFolders(realEnvironment);
    if (process.argv.includes("--prove-only")) {
      // The proof alone: nothing is migrated and the application is not started.
      console.log("native-smoke: proof only, the application was not started");
      return;
    }

    // 2. The database and the generated root, as the e2e runner prepares them.
    // The console steps get the run's temporary directory too (SQLite temp files stay in the root).
    const database = { JOBNIAC_DB_PATH: run.databaseFile, ...run.isolation };
    const migrated = runCli(["migrate"], sessionEnvironment(process.env, database));
    if (migrated.status !== 0) {
      throw new Error(`migrate failed with exit code ${migrated.status}`);
    }
    const setup = runCli(
      ["setup-generated-root"],
      sessionEnvironment(process.env, { ...database, JOBNIAC_GENERATED_ROOT: run.generatedRoot }),
    );
    if (setup.status !== 0) {
      throw new Error(`setup-generated-root failed with exit code ${setup.status}`);
    }

    // 3. The launch.
    console.log("native-smoke: launching the application");
    const result = await launch(environment);
    console.log(`native-smoke: exit code ${result.code} signal ${result.signal}`);
    if (result.output.trim() !== "") {
      console.log(`native-smoke: output tail:\n${result.output}`);
    }

    // 4. The proof after the run, and what the run wrote under the root.
    for (const fact of verifyAfterRun({ run, before, realEnvironment })) {
      console.log(`native-smoke: after run: ${fact}`);
    }
    console.log(`native-smoke: written under the run root: ${JSON.stringify(observeRunRoot(run))}`);
    if (result.code !== 0) {
      throw new Error("the application did not exit with status 0");
    }
    console.log("native-smoke: PASS");
  } catch (error) {
    fail(error.message);
  } finally {
    try {
      rmSync(run.root, { recursive: true, force: true });
    } catch (error) {
      console.error(`native-smoke: could not remove ${run.root}: ${error.message}`);
    }
  }
}

await main();
