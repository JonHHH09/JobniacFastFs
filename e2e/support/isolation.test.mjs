// Unit tests of the run isolation proof (no application and no driver are started):
//
//   node --test e2e/support/isolation.test.mjs
import assert from "node:assert/strict";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { after, describe, it } from "node:test";
import { createRunRoot, repoRoot } from "./config.mjs";
import {
  LAYOUT,
  MARKER_FILE,
  proveBeforeLaunch as proveBeforeLaunchOnBinary,
  REPORT_FILE,
  snapshotRealFolders,
  verifyAfterRun,
} from "./isolation.mjs";

const windows = process.platform === "win32";
const identifier = JSON.parse(
  readFileSync(join(repoRoot, "src-tauri", "tauri.conf.json"), "utf8"),
).identifier;
const debugBinary = join(repoRoot, "target", "debug", windows ? "jobniac.exe" : "jobniac");
const releaseBinary = join(repoRoot, "target", "release", windows ? "jobniac.exe" : "jobniac");

// A synthetic user session that is not the real one and does not exist on disk.
const syntheticSession = windows
  ? {
      APPDATA: "Z:\\synthetic\\AppData\\Roaming",
      LOCALAPPDATA: "Z:\\synthetic\\AppData\\Local",
      USERPROFILE: "Z:\\synthetic",
    }
  : {
      XDG_CONFIG_HOME: "/synthetic/.config",
      XDG_DATA_HOME: "/synthetic/.local/share",
      XDG_CACHE_HOME: "/synthetic/.cache",
      HOME: "/synthetic",
    };

// The proof reads the application binary for the gate name; the tests inject its bytes.
const gatedBinary = () => Buffer.from("....JOBNIAC_TEST_RUN_ROOT....");
const ungatedBinary = () => Buffer.from("a binary without the gate");
function proveBeforeLaunch(arguments_) {
  return proveBeforeLaunchOnBinary({ readBinary: gatedBinary, ...arguments_ });
}

const created = [];
after(() => {
  for (const directory of created) {
    rmSync(directory, { recursive: true, force: true });
  }
});

function newRun() {
  const run = createRunRoot();
  created.push(run.root);
  const env = {
    JOBNIAC_DB_PATH: run.databaseFile,
    JOBNIAC_GENERATED_ROOT: run.generatedRoot,
    JOBNIAC_DOCUMENT_IMPORT_ROOT: run.importRoot,
    ...run.isolation,
  };
  return { run, env };
}

function prove(overrides = {}) {
  const { run, env } = newRun();
  return proveBeforeLaunch({
    run,
    appBinaryPath: debugBinary,
    env,
    realEnvironment: syntheticSession,
    ...overrides,
  });
}

describe("proveBeforeLaunch", () => {
  it("accepts a prepared run root with every path confined", () => {
    const facts = prove();
    assert.ok(facts.length >= 8, facts.join("\n"));
  });

  it("refuses a release binary, which compiles the isolation out", () => {
    assert.throws(() => prove({ appBinaryPath: releaseBinary }), /debug build folder/);
  });

  it("refuses a binary without the gate (a release or a stale build) before it is started", () => {
    assert.throws(() => prove({ readBinary: ungatedBinary }), /carries the isolation gate/);
  });

  it("refuses a root without the marker", () => {
    const { run, env } = newRun();
    rmSync(join(run.root, MARKER_FILE));
    assert.throws(
      () =>
        proveBeforeLaunch({
          run,
          appBinaryPath: debugBinary,
          env,
          realEnvironment: syntheticSession,
        }),
      /run marker/,
    );
  });

  it("refuses a root that is not under target/", () => {
    const { run, env } = newRun();
    assert.throws(
      () =>
        proveBeforeLaunch({
          run: { ...run, root: dirname(repoRoot) },
          appBinaryPath: debugBinary,
          env: { ...env, JOBNIAC_TEST_RUN_ROOT: dirname(repoRoot) },
          realEnvironment: syntheticSession,
        }),
      /under target/,
    );
  });

  it("refuses a database, generated root or temporary directory outside the root", () => {
    const outside = join(dirname(repoRoot), "elsewhere");
    const names = [
      "JOBNIAC_DB_PATH",
      "JOBNIAC_GENERATED_ROOT",
      ...(windows ? ["TMP", "TEMP"] : ["TMPDIR"]),
    ];
    for (const name of names) {
      const { run, env } = newRun();
      assert.throws(
        () =>
          proveBeforeLaunch({
            run,
            appBinaryPath: debugBinary,
            env: { ...env, [name]: outside },
            realEnvironment: syntheticSession,
          }),
        new RegExp(`${name} is inside the run root`),
        name,
      );
    }
  });

  it("refuses a run root that contains a real per-user base", () => {
    const { run, env } = newRun();
    const base = windows ? "USERPROFILE" : "HOME";
    assert.throws(
      () =>
        proveBeforeLaunch({
          run,
          appBinaryPath: debugBinary,
          env,
          realEnvironment: { ...syntheticSession, [base]: join(run.root, "inner") },
        }),
      /real per-user base/,
    );
  });

  it("refuses a run root inside a real application folder", () => {
    // `<base>/<identifier>` of the synthetic session contains a root made there.
    const base = join(repoRoot, "target", "synthetic-base");
    const root = join(base, identifier, "e2e-fake");
    mkdirSync(root, { recursive: true });
    created.push(base);
    writeFileSync(join(root, MARKER_FILE), "jobniac-test-run-v1\n");
    const key = windows ? "APPDATA" : "XDG_CONFIG_HOME";
    const tmp = join(root, "tmp");
    const env = {
      JOBNIAC_TEST_RUN_ROOT: root,
      JOBNIAC_DB_PATH: join(root, "db.sqlite3"),
      JOBNIAC_GENERATED_ROOT: join(root, "generated"),
      ...(windows ? { TMP: tmp, TEMP: tmp } : { TMPDIR: tmp }),
    };
    assert.throws(
      () =>
        proveBeforeLaunch({
          run: { root },
          appBinaryPath: debugBinary,
          env,
          realEnvironment: { ...syntheticSession, [key]: base },
        }),
      /inside a real application folder/,
    );
  });
});

describe("verifyAfterRun", () => {
  function reportFor(run, patch = {}) {
    const report = { verified: true };
    for (const [key, leaf] of Object.entries(LAYOUT)) {
      report[key] = join(run.root, leaf);
    }
    writeFileSync(join(run.root, REPORT_FILE), JSON.stringify({ ...report, ...patch }));
  }

  it("fails when the application wrote no report", () => {
    const { run } = newRun();
    assert.throws(
      () => verifyAfterRun({ run, before: {}, realEnvironment: syntheticSession }),
      /no isolation report/,
    );
  });

  it("fails when the report is not verified or names another directory", () => {
    const { run } = newRun();
    reportFor(run, { verified: false });
    assert.throws(
      () => verifyAfterRun({ run, before: {}, realEnvironment: syntheticSession }),
      /did not report a verified/,
    );
    reportFor(run, { appLocalDataDir: join(dirname(repoRoot), "real") });
    assert.throws(
      () => verifyAfterRun({ run, before: {}, realEnvironment: syntheticSession }),
      /appLocalDataDir is not the planned directory/,
    );
  });

  it("fails when a real application folder changed, passes when it did not", () => {
    const { run } = newRun();
    reportFor(run);
    const real = snapshotRealFolders(syntheticSession);
    const facts = verifyAfterRun({ run, before: real, realEnvironment: syntheticSession });
    // One fact per watched folder (the synthetic session's and the real home's) plus the five
    // planned directories.
    assert.equal(facts.length, Object.keys(real).length + Object.keys(LAYOUT).length);
    assert.ok(Object.keys(real).length >= 2);
    const [folder] = Object.keys(real);
    assert.throws(
      () =>
        verifyAfterRun({
          run,
          before: { [folder]: ["file|1|1"] },
          realEnvironment: syntheticSession,
        }),
      /real application folder changed/,
    );
  });
});
