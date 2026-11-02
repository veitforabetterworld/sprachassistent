import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { API_KEYS_SCHEMA_SQL } from "@veit/api-keys/pg";
import { AUTH_SCHEMA_SQL } from "@veit/auth-server/pg";
import { createPool } from "@veit/db";
import { ROOT, env } from "./env.js";

export const pool = createPool({ connectionString: env.databaseUrl });

/** Same number as SCHEMA_LOCK in src/store.rs: the voice server applies db/schema.sql too. */
const SCHEMA_LOCK = 7_351_402;

/** Account tables of the framework, the admin flag and the tables of db/schema.sql. */
export async function ensureSchema(): Promise<void> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("SELECT pg_advisory_xact_lock($1)", [SCHEMA_LOCK]);
    await client.query(AUTH_SCHEMA_SQL);
    await client.query(`ALTER TABLE users ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT false`);
    await client.query(API_KEYS_SCHEMA_SQL);
    await client.query(readFileSync(resolve(ROOT, "db/schema.sql"), "utf8"));
    await client.query(
      `UPDATE devices SET user_id = (SELECT id FROM users WHERE is_admin ORDER BY id LIMIT 1)
       WHERE user_id IS NULL AND EXISTS (SELECT 1 FROM users WHERE is_admin)`,
    );
    await client.query(
      `UPDATE llm_providers SET user_id = (SELECT id FROM users WHERE is_admin ORDER BY id LIMIT 1)
       WHERE user_id IS NULL AND EXISTS (SELECT 1 FROM users WHERE is_admin)`,
    );
    await client.query(`
      DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'devices_user_fk') THEN
          ALTER TABLE devices ADD CONSTRAINT devices_user_fk FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'llm_providers_user_fk') THEN
          ALTER TABLE llm_providers ADD CONSTRAINT llm_providers_user_fk FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'mcp_servers_user_fk') THEN
          ALTER TABLE mcp_servers ADD CONSTRAINT mcp_servers_user_fk FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'agent_runs_user_fk') THEN
          ALTER TABLE agent_runs ADD CONSTRAINT agent_runs_user_fk FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
        END IF;
      END $$`);
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}
