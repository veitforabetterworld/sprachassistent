import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { API_KEYS_SCHEMA_SQL } from "@veit/api-keys/pg";
import { AUTH_SCHEMA_SQL } from "@veit/auth-server/pg";
import { pool } from "./db.js";
import { ROOT } from "./env.js";

/**
 * The one place that changes the database: at deploy (deployments/pi/runtime-reload.sh), before the servers
 * restart, and before `pnpm dev:api`. The servers only check that nothing is missing (`pendingMigrations`).
 *
 * db/migrations/NNNN_name.sql run once each, in name order, and are recorded in schema_migrations. A
 * migration that has run is never edited; a change gets a new file. The account tables belong to the
 * framework, which keeps its own SQL idempotent, so it runs first every time.
 */

const MIGRATIONS = resolve(ROOT, "db/migrations");
/** Two deploys at once wait for each other. */
const MIGRATION_LOCK = 7_351_402;

export function migrationNames(): string[] {
  return readdirSync(MIGRATIONS)
    .filter((name) => /^\d{4}_[a-z0-9_]+\.sql$/.test(name))
    .sort();
}

/** Applies what is missing, all in one transaction. Returns the names it applied. */
export async function migrate(): Promise<string[]> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("SELECT pg_advisory_xact_lock($1)", [MIGRATION_LOCK]);
    await client.query(AUTH_SCHEMA_SQL);
    await client.query(API_KEYS_SCHEMA_SQL);
    await client.query(
      `CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW())`,
    );
    const done = new Set((await client.query<{ name: string }>(`SELECT name FROM schema_migrations`)).rows.map((r) => r.name));
    const applied: string[] = [];
    for (const name of migrationNames()) {
      if (done.has(name)) continue;
      await client.query(readFileSync(resolve(MIGRATIONS, name), "utf8"));
      await client.query(`INSERT INTO schema_migrations (name) VALUES ($1)`, [name]);
      applied.push(name);
    }
    await client.query("COMMIT");
    return applied;
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}

/** Migrations the database has not seen; the server refuses to start with any. */
export async function pendingMigrations(): Promise<string[]> {
  const table = await pool.query<{ exists: string | null }>(`SELECT to_regclass('schema_migrations') AS exists`);
  if (!table.rows[0]?.exists) return migrationNames();
  const done = new Set((await pool.query<{ name: string }>(`SELECT name FROM schema_migrations`)).rows.map((r) => r.name));
  return migrationNames().filter((name) => !done.has(name));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const applied = await migrate();
  console.log(applied.length ? `Migriert: ${applied.join(", ")}` : "Datenbank ist aktuell.");
  await pool.end();
}
