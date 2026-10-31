// End-to-end runner: migrates a temporary SQLite database file, registers a temporary generated
// root and runs the application sessions in order (default, opt-out, generation):
//
//   node e2e/run.mjs
//
// The database file lives in the run folder under target/ and is removed with it. See
// e2e/README.md for the build steps and the environment.
import { spawnSync } from "node:child_process";
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

// The flags of the optional features. The desktop app has them on unless the process sets one: any
// value other than `1` (here `0`) turns the feature off and wins over the Settings switch.
const FEATURE_FLAGS = [
  "JOBNIAC_ARBEITNOW_ENABLED",
  "JOBNIAC_LINK_FETCH_ENABLED",
  "JOBNIAC_DELETION_ENABLED",
  "JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED",
  "JOBNIAC_GENERATED_ENABLED",
];

const SESSIONS = [
  { name: "default", spec: "e2e/specs/app.spec.mjs" },
  { name: "opt-out", spec: "e2e/specs/optout.spec.mjs" },
  { name: "generation", spec: "e2e/specs/generation.spec.mjs" },
];

function fail(message) {
  console.error(`e2e: ${message}`);
  process.exit(2);
}

// A failed precondition inside the try block: log it and return, so the finally clause still
// removes the run root (process.exit would skip it).
function failInRun(message) {
  console.error(`e2e: ${message}`);
  process.exitCode = 2;
}

function main() {
  if (!existsSync(appBinary)) {
    fail(
      `the desktop binary is missing at ${appBinary}; run pnpm tauri build --debug --no-bundle first`,
    );
  }
  if (!existsSync(cliBinary)) {
    fail(`the console binary is missing at ${cliBinary}; run cargo build -p jobniac-cli first`);
  }

  const run = createRunRoot();
  const database = { JOBNIAC_DB_PATH: run.databaseFile };
  const pdfiumDirectory = join(
    repoRoot,
    "vendor",
    "pdfium",
    process.platform === "win32"
      ? "windows-x64"
      : process.platform === "darwin"
        ? process.arch === "arm64"
          ? "mac-arm64"
          : "mac-x64"
        : "linux-x64",
  );
  const shared = {
    ...database,
    ...run.userDirectories,
    JOBNIAC_DOCUMENT_IMPORT_ROOT: run.importRoot,
    JOBNIAC_E2E_GENERATED_ROOT: run.generatedRoot,
    // Generation is on by default, so every session names the run's root: the app never registers
    // the per-user default root of the machine.
    JOBNIAC_GENERATED_ROOT: run.generatedRoot,
    ...(existsSync(pdfiumDirectory) ? { JOBNIAC_PDFIUM_DIR: pdfiumDirectory } : {}),
  };

  let exitCode = 0;
  try {
    const migrated = runCli(["migrate"], sessionEnvironment(process.env, database));
    if (migrated.status !== 0) {
      failInRun(`migrate failed with exit code ${migrated.status}: ${migrated.stderr.trim()}`);
      return;
    }
    const setup = runCli(
      ["setup-generated-root"],
      sessionEnvironment(process.env, { ...database, JOBNIAC_GENERATED_ROOT: run.generatedRoot }),
    );
    if (setup.status !== 0) {
      failInRun(
        `setup-generated-root failed with exit code ${setup.status}: ${setup.stderr.trim()}`,
      );
      return;
    }

    const mocha = join(repoRoot, "node_modules", "mocha", "bin", "mocha.js");
    for (const session of SESSIONS) {
      let extra = shared;
      if (session.name === "opt-out") {
        extra = { ...shared, ...Object.fromEntries(FEATURE_FLAGS.map((flag) => [flag, "0"])) };
      } else if (session.name === "generation") {
        extra = { ...shared, JOBNIAC_GENERATED_ENABLED: "1" };
      }
      console.log(`e2e: running the ${session.name} session`);
      const result = spawnSync(
        process.execPath,
        [mocha, "--timeout", "180000", "--reporter", "spec", session.spec],
        {
          cwd: repoRoot,
          env: sessionEnvironment(process.env, extra),
          stdio: "inherit",
          windowsHide: true,
        },
      );
      if (result.status !== 0) {
        exitCode = result.status ?? 1;
      }
    }
  } finally {
    try {
      rmSync(run.root, { recursive: true, force: true });
    } catch (error) {
      console.error(`e2e: could not remove ${run.root}: ${error.message}`);
    }
  }
  process.exitCode = exitCode;
}

main();
