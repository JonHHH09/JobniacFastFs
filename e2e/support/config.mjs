// Paths, environment and temporary directories shared by the runner and the specs.
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

const exeSuffix = process.platform === "win32" ? ".exe" : "";

/** The debug desktop binary (`pnpm tauri build --debug --no-bundle`). */
export const appBinary =
  process.env.JOBNIAC_E2E_APP ?? join(repoRoot, "target", "debug", `jobniac${exeSuffix}`);

/** The console binary that runs `migrate` and `setup-generated-root` (`cargo build -p jobniac-cli`). */
export const cliBinary =
  process.env.JOBNIAC_E2E_CLI ?? join(repoRoot, "target", "debug", `jobniac-cli${exeSuffix}`);

/** Ignored by git (see e2e/.gitignore): failure screenshots go here. */
export const artifactsDir = join(repoRoot, "e2e", ".artifacts");

/** Environment variables of the application that the suite controls. Everything else named JOBNIAC_* is dropped. */
const DATABASE_KEYS = [
  "JOBNIAC_DB_HOST",
  "JOBNIAC_DB_PORT",
  "JOBNIAC_DB_NAME",
  "JOBNIAC_DB_USER",
  "JOBNIAC_DB_PASSWORD",
];
const SUITE_KEYS = [
  "JOBNIAC_DOCUMENT_IMPORT_ROOT",
  "JOBNIAC_E2E_ROOT",
  "JOBNIAC_E2E_GENERATED_ROOT",
];

/**
 * The environment of one application session. No opt-in flag is set unless the session asks for it:
 * the default session runs with every opt-in screen off.
 */
export function sessionEnvironment(base, extra = {}) {
  const env = {};
  for (const [key, value] of Object.entries(base)) {
    if (!key.startsWith("JOBNIAC_") || DATABASE_KEYS.includes(key) || SUITE_KEYS.includes(key)) {
      env[key] = value;
    }
  }
  return { ...env, ...extra };
}

/** Reads the disposable-database variables; the runner refuses to start without them. */
export function databaseEnvironment(base) {
  return Object.fromEntries(DATABASE_KEYS.map((key) => [key, base[key] ?? ""]));
}

/**
 * Creates the directories of one run: the import root and the generated root. They live under the
 * ignored `target/` folder of the repository and are removed by the runner when it ends. The
 * generated root is not placed in the user temporary folder: setup-generated-root refuses that
 * location on Windows (see e2e/README.md).
 */
export function createRunRoot() {
  mkdirSync(join(repoRoot, "target"), { recursive: true });
  const root = mkdtempSync(join(repoRoot, "target", "e2e-"));
  const importRoot = join(root, "imports");
  const generatedRoot = join(root, "generated");
  mkdirSync(importRoot);
  // The generated root must be private, or setup-generated-root refuses it. Linux: owner-only mode
  // bits. Windows ignores the mode, so the folder gets a protected DACL instead.
  mkdirSync(generatedRoot, { mode: 0o700 });
  if (process.platform === "win32") {
    protectWindowsDirectory(generatedRoot);
  }
  return { root, importRoot, generatedRoot };
}

/**
 * Replaces the inherited ACL of a Windows directory with a protected one for the current user,
 * SYSTEM and Administrators, the principals the setup check trusts. Without this the folder
 * inherits the ACL of its parent, which on the CI runner (workspace on D:) carries an
 * Authenticated Users write grant that setup-generated-root rightly refuses as a shared directory.
 */
function protectWindowsDirectory(directory) {
  const whoami = execFileSync("whoami", ["/user", "/fo", "csv", "/nh"], { encoding: "utf8" });
  const sid = /"(S-1-[0-9-]+)"/.exec(whoami)?.[1];
  if (!sid) {
    throw new Error("the current user SID could not be read");
  }
  const grants = [sid, "S-1-5-18", "S-1-5-32-544"].map((principal) => `*${principal}:(OI)(CI)F`);
  execFileSync("icacls", [directory, "/inheritance:r", "/grant:r", ...grants], {
    stdio: ["ignore", "ignore", "inherit"],
  });
}
