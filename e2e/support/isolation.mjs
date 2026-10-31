// Run isolation of the native suites: every per-user directory of the debug application is
// redirected into the run root, the redirection is proven before the application starts, and the
// real folders are shown unchanged afterwards.
//
// The redirection itself is in the debug build (src-tauri/src/test_isolation.rs): it reads
// JOBNIAC_TEST_RUN_ROOT, redirects the five Tauri app directories (config, data, local data, cache,
// log; on Windows the Known Folder APIs ignore APPDATA and LOCALAPPDATA, and the WebView2 profile
// lives in the local data directory) and refuses to start when anything is not confined. A release
// build ignores the variable, so the runner only accepts a debug binary (see `proveBeforeLaunch`).
import { existsSync, lstatSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { isInsideRunArea, repoRoot } from "./config.mjs";

export const ROOT_VARIABLE = "JOBNIAC_TEST_RUN_ROOT";
export const MARKER_FILE = ".jobniac-test-run";
export const MARKER_TEXT = "jobniac-test-run-v1\n";
export const REPORT_FILE = "isolation-report.json";

/** The directories under the run root; the same names as test_isolation.rs. */
export const LAYOUT = {
  appConfigDir: "app-config",
  appDataDir: "app-data",
  appLocalDataDir: "app-local-data",
  appCacheDir: "app-cache",
  appLogDir: "app-log",
};

/** Writes the marker without which the application refuses the root. */
export function writeMarker(root) {
  writeFileSync(join(root, MARKER_FILE), MARKER_TEXT);
}

/**
 * The environment that pins the run into `root`: the root variable, and the temporary directory
 * (TMP and TEMP on Windows, TMPDIR elsewhere) that the PDF worker and the drivers inherit.
 */
export function isolationEnvironment(root, tmp) {
  const temporary = process.platform === "win32" ? { TMP: tmp, TEMP: tmp } : { TMPDIR: tmp };
  return { [ROOT_VARIABLE]: root, ...temporary };
}

/** Whether `child` is `parent` or inside it, by lexical resolution (case-insensitive on Windows). */
function isInside(parent, child) {
  const fold = (value) => (process.platform === "win32" ? value.toLowerCase() : value);
  const relation = relative(fold(resolve(parent)), fold(resolve(child)));
  return relation === "" || (!relation.startsWith("..") && !isAbsolute(relation));
}

function isStrictlyInside(parent, child) {
  return resolve(parent) !== resolve(child) && isInside(parent, child);
}

/** The identifier of the application: the name of its folder under each per-user base. */
function identifier() {
  const config = JSON.parse(readFileSync(join(repoRoot, "src-tauri", "tauri.conf.json"), "utf8"));
  return String(config.identifier);
}

/**
 * The roaming and local bases a profile has. They come from the process environment AND from the
 * home folder: the Known Folder APIs the application uses ignore APPDATA and LOCALAPPDATA, so a
 * session whose variables are redirected must still have the real folders watched.
 */
function realDataBases(environment) {
  const bases =
    process.platform === "win32"
      ? [
          environment.APPDATA,
          environment.LOCALAPPDATA,
          join(homedir(), "AppData", "Roaming"),
          join(homedir(), "AppData", "Local"),
        ]
      : [
          environment.XDG_CONFIG_HOME,
          environment.XDG_DATA_HOME,
          environment.XDG_CACHE_HOME,
          join(homedir(), ".config"),
          join(homedir(), ".local", "share"),
          join(homedir(), ".cache"),
        ];
  return [...new Set(bases.filter((value) => value && isAbsolute(value)))];
}

/** The real per-user bases of the machine. */
function realBases(environment) {
  const home = process.platform === "win32" ? environment.USERPROFILE : environment.HOME;
  return [...new Set([...realDataBases(environment), home, homedir()].filter(Boolean))];
}

/**
 * The real application folders: `<base>/<identifier>` of every data base, and on Windows the
 * default per-user data folder of the database and the generated root (`Local\Jobniac`).
 */
function realApplicationFolders(environment) {
  const id = identifier();
  const folders = realDataBases(environment).map((base) => join(base, id));
  if (process.platform === "win32") {
    folders.push(
      ...realDataBases(environment)
        .filter((base) => /local$/i.test(base))
        .map((base) => join(base, "Jobniac")),
    );
  }
  return [...new Set(folders)];
}

/**
 * Whether the binary carries the isolation gate (the variable name is in its text). A release
 * build, or a debug build made before the gate existed, does not, and would run against the real
 * folders: it is refused before it is started.
 */
export function binaryHasGate(binaryPath, readBinary = readFileSync) {
  return readBinary(binaryPath).includes(ROOT_VARIABLE);
}

/**
 * The proof before the application starts. It throws (and the caller must not launch) unless:
 * the binary is a debug build, the run root is a prepared folder under `target/`, it overlaps no
 * real per-user folder, and every path the application will use is inside it. Returns the lines
 * that were checked, for the log.
 *
 * `realEnvironment` is the process environment as the user's session has it (not the run's).
 */
export function proveBeforeLaunch({
  run,
  appBinaryPath,
  env,
  realEnvironment,
  readBinary = readFileSync,
}) {
  const facts = [];
  const check = (condition, message) => {
    if (!condition) {
      throw new Error(`isolation: ${message}`);
    }
    facts.push(message);
  };

  // A release build compiles the isolation out and would use the real folders.
  check(
    /[\\/]debug[\\/][^\\/]+$/.test(resolve(appBinaryPath)),
    "the application binary sits in a debug build folder",
  );
  check(
    binaryHasGate(appBinaryPath, readBinary),
    "the application binary carries the isolation gate (not a release or a stale build)",
  );
  check(isInsideRunArea(run.root), "the run root is under target/ of the repository");
  check(
    existsSync(join(run.root, MARKER_FILE)) &&
      readFileSync(join(run.root, MARKER_FILE), "utf8") === MARKER_TEXT,
    "the run root holds the exact run marker",
  );
  check(env[ROOT_VARIABLE] === run.root, "the application is told the run root");

  for (const base of realBases(realEnvironment)) {
    check(
      !isInside(run.root, base),
      "the run root is not a real per-user base nor does it contain one",
    );
  }
  for (const folder of realApplicationFolders(realEnvironment)) {
    check(!isInside(folder, run.root), "the run root is not inside a real application folder");
  }

  const pinned = ["JOBNIAC_DB_PATH", "JOBNIAC_GENERATED_ROOT"];
  pinned.push(...(process.platform === "win32" ? ["TMP", "TEMP"] : ["TMPDIR"]));
  for (const name of pinned) {
    check(
      typeof env[name] === "string" && isStrictlyInside(run.root, env[name]),
      `${name} is inside the run root`,
    );
  }
  for (const name of ["JOBNIAC_DOCUMENT_IMPORT_ROOT", "WEBVIEW2_USER_DATA_FOLDER"]) {
    check(
      env[name] === undefined || isStrictlyInside(run.root, env[name]),
      `${name} is unset or inside the run root`,
    );
  }
  return facts;
}

/** What is inside `directory` (relative path, size, mtime), or null when it does not exist. */
function listTree(directory, limit = 50_000) {
  if (!existsSync(directory)) {
    return null;
  }
  const entries = [];
  for (const entry of readdirSync(directory, { recursive: true, withFileTypes: true })) {
    if (entries.length >= limit) {
      entries.push("(truncated)");
      break;
    }
    const full = join(entry.parentPath, entry.name);
    try {
      const stat = lstatSync(full);
      entries.push(`${relative(directory, full)}|${stat.size}|${stat.mtimeMs}`);
    } catch {
      entries.push(`${relative(directory, full)}|gone`);
    }
  }
  return entries.sort();
}

/**
 * A metadata snapshot (names, sizes, modification times; no file is opened) of the real
 * application folders. Taken before and after the run; compared by `verifyAfterRun`.
 */
export function snapshotRealFolders(realEnvironment) {
  const snapshot = {};
  for (const folder of realApplicationFolders(realEnvironment)) {
    snapshot[folder] = listTree(folder);
  }
  return snapshot;
}

/**
 * The proof after the run: the application's own report says it verified the redirection and its
 * resolved directories are the planned ones; the real folders are as they were. Returns the
 * evidence lines, and throws on the first failure.
 */
export function verifyAfterRun({ run, before, realEnvironment }) {
  const facts = [];
  const reportPath = join(run.root, REPORT_FILE);
  if (!existsSync(reportPath)) {
    throw new Error(
      "isolation: the application wrote no isolation report (a release build, or it never started)",
    );
  }
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  if (report.verified !== true) {
    throw new Error("isolation: the application did not report a verified isolation");
  }
  for (const [key, leaf] of Object.entries(LAYOUT)) {
    if (resolve(report[key]) !== resolve(join(run.root, leaf))) {
      throw new Error(`isolation: ${key} is not the planned directory`);
    }
    facts.push(`${key} resolved to <run root>/${leaf}`);
  }
  const after = snapshotRealFolders(realEnvironment);
  for (const [folder, listing] of Object.entries(before)) {
    if (JSON.stringify(listing) !== JSON.stringify(after[folder])) {
      throw new Error(`isolation: a real application folder changed during the run (${folder})`);
    }
    facts.push(
      listing === null
        ? "a real application folder did not exist before and does not exist after"
        : `a real application folder is unchanged (${listing.length} entries)`,
    );
  }
  return facts;
}

/** Observations of what the run wrote under the root, for the report (not requirements). */
export function observeRunRoot(run) {
  const observed = {};
  for (const [key, leaf] of Object.entries(LAYOUT)) {
    const directory = join(run.root, leaf);
    observed[key] = listTree(directory)?.map((line) => line.split("|")[0]) ?? null;
  }
  return observed;
}
