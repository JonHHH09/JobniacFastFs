// Database helpers: the console binary for migrations and the generated root, psql lookups of the
// identifiers the screens take as input, and a table lock for the busy-state test. The suite only
// talks to the disposable cluster that `pg-run` starts (see e2e/README.md); it refuses any other host.
import { spawn, spawnSync } from "node:child_process";
import { cliBinary } from "./config.mjs";

const DISPOSABLE_HOST = "127.0.0.1";

/** Throws unless the environment points at the disposable loopback cluster from `pg-run`. */
export function assertDisposableDatabase(env) {
  if (env.JOBNIAC_DB_HOST !== DISPOSABLE_HOST || !env.PGPASSWORD || env.PGDATABASE !== "postgres") {
    throw new Error(
      "e2e: no disposable database in the environment; run the suite through pg-run " +
        "(pg-run node e2e/run.mjs)",
    );
  }
}

/** Runs the console binary with the given arguments and the database environment; returns its result. */
export function runCli(args, env) {
  return spawnSync(cliBinary, args, { env, encoding: "utf8", timeout: 120_000, windowsHide: true });
}

/** One value from psql, or throws. The query is fixed by the suite, never taken from a page. */
export function queryScalar(sql, env) {
  const result = spawnSync(
    "psql",
    ["--no-psqlrc", "--tuples-only", "--no-align", "--set", "ON_ERROR_STOP=1", "--command", sql],
    { env, encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  const value = result.stdout.trim();
  if (result.status !== 0 || value === "") {
    throw new Error(
      `e2e: psql lookup returned no value (exit ${result.status}, ${result.error?.message ?? ""} ${result.stderr.trim()})`,
    );
  }
  return value;
}

/**
 * Holds an ACCESS EXCLUSIVE lock on `table` in a separate psql session until that process is ended.
 * It makes a database call wait for as long as the test needs (the busy-state test).
 */
export function holdTableLock(table, env) {
  const script = `BEGIN; LOCK TABLE ${table} IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(300); COMMIT;`;
  return spawn("psql", ["--no-psqlrc", "--set", "ON_ERROR_STOP=1", "--command", script], {
    env,
    stdio: "ignore",
    windowsHide: true,
  });
}

/** Ends the sessions that hold an ACCESS EXCLUSIVE lock on the relation `relation`. */
export function releaseTableLock(relation, env) {
  const sql =
    "select pg_terminate_backend(l.pid) from pg_locks l join pg_class c on c.oid = l.relation " +
    `where c.relname = '${relation}' and l.mode = 'AccessExclusiveLock' and l.granted`;
  spawnSync("psql", ["--no-psqlrc", "--tuples-only", "--no-align", "--command", sql], {
    env,
    encoding: "utf8",
    timeout: 30_000,
    windowsHide: true,
  });
}

/** Whether a granted ACCESS EXCLUSIVE lock on the relation named `relation` exists. */
export function isLockHeld(relation, env) {
  const sql =
    "select count(*) from pg_locks l join pg_class c on c.oid = l.relation " +
    `where c.relname = '${relation}' and l.mode = 'AccessExclusiveLock' and l.granted`;
  return queryScalar(sql, env) !== "0";
}
