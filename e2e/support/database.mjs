// Database helpers: the console binary for migrations and the generated root, read-only lookups of
// the identifiers the screens take as input, and a write lock for the busy-state test. The suite
// only touches the temporary SQLite file of the run (`JOBNIAC_DB_PATH`, created under `target/` by
// run.mjs); it refuses any other location. Lookups use `node:sqlite` (Node 24), so no sqlite3
// program is needed on the runners.
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { cliBinary, isInsideRunArea } from "./config.mjs";

/** The database file of this run; throws unless it is set and inside the run area under `target/`. */
export function databasePath(env) {
  const path = env.JOBNIAC_DB_PATH;
  if (!path || !isInsideRunArea(path)) {
    throw new Error(
      "e2e: no temporary database in the environment; run the suite through run.mjs " +
        "(node e2e/run.mjs)",
    );
  }
  return path;
}

/** Runs the console binary with the given arguments and the database environment; returns its result. */
export function runCli(args, env) {
  return spawnSync(cliBinary, args, { env, encoding: "utf8", timeout: 120_000, windowsHide: true });
}

/** One value from a read-only lookup, or throws. The query is fixed by the suite, never taken from a page. */
export function queryScalar(sql, env, ...parameters) {
  const database = new DatabaseSync(databasePath(env), { readOnly: true });
  try {
    database.exec("PRAGMA busy_timeout = 10000");
    const row = database.prepare(sql).get(...parameters);
    const value = row === undefined ? undefined : Object.values(row)[0];
    if (value === undefined || value === null || String(value) === "") {
      throw new Error("e2e: the database lookup returned no value");
    }
    return String(value);
  } finally {
    database.close();
  }
}

/**
 * Takes the database write lock (`BEGIN IMMEDIATE`) in this process and returns a function that
 * releases it. A write of the application waits meanwhile (its busy timeout is 10 seconds), which
 * makes the busy state of a control deterministic. Readers are not blocked: the file is in WAL mode.
 */
export function holdWriteLock(env) {
  const database = new DatabaseSync(databasePath(env));
  database.exec("PRAGMA busy_timeout = 10000");
  database.exec("BEGIN IMMEDIATE");
  let released = false;
  return () => {
    if (released) {
      return;
    }
    released = true;
    try {
      database.exec("ROLLBACK");
    } finally {
      database.close();
    }
  };
}
