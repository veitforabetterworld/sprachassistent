import { existsSync } from "node:fs";
import { resolve } from "node:path";

/** Repository root, from src/ (tsx) and dist/ (node) alike. */
export const ROOT = resolve(import.meta.dirname, "../../..");

// Only the database connection is configured here; every other setting lives in the database.
const envFile = resolve(ROOT, ".env");
if (existsSync(envFile)) process.loadEnvFile(envFile);

export const env = {
  databaseUrl:
    process.env.DATABASE_URL ?? "postgres://sprachassistent:sprachassistent@localhost:5432/sprachassistent",
  host: process.env.WEB_API_HOST ?? "127.0.0.1",
  port: Number(process.env.WEB_API_PORT ?? 3022),
  /** The Rust voice server; its /internal routes are not exposed by nginx. */
  voiceUrl: process.env.VOICE_URL ?? "http://127.0.0.1:3020",
};
