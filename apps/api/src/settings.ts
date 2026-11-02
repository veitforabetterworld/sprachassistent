import { randomBytes } from "node:crypto";
import { pool } from "./db.js";
import { credentialsEnabled, encrypt, openSecret, seal } from "./secrets.js";

/** Generated once, used by the server only and never sent to the browser. */
const SECRET_KEYS = ["api_key_pepper", "api_key_secret", "auth_mfa_key"] as const;
/** Write-only in the admin area: GET says whether they are set, an empty value on PUT keeps them. */
export const PASSWORD_SETTINGS = ["smtp_password", "google_oauth_client_secret"] as const;

export async function getSetting(key: string): Promise<string | null> {
  const result = await pool.query<{ value: string }>(`SELECT value FROM app_settings WHERE key = $1`, [key]);
  const value = result.rows[0]?.value;
  if (value == null) return null;
  return openSecret(value);
}

export async function setSetting(key: string, value: string): Promise<void> {
  await pool.query(
    `INSERT INTO app_settings (key, value, updated_at) VALUES ($1, $2, NOW())
     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()`,
    [key, value],
  );
}

/** Creates the secret on first use. */
export async function ensureSecret(key: (typeof SECRET_KEYS)[number]): Promise<string> {
  const existing = await getSetting(key);
  if (existing) return existing;
  const value = randomBytes(32).toString("base64url");
  await setSetting(key, value);
  return value;
}

/** Every setting the admin area may see and change. */
export async function listSettings(): Promise<Record<string, string | boolean>> {
  const result = await pool.query<{ key: string; value: string }>(`SELECT key, value FROM app_settings ORDER BY key`);
  const values: Record<string, string | boolean> = {};
  for (const { key, value } of result.rows) {
    if ((SECRET_KEYS as readonly string[]).includes(key)) continue;
    if ((PASSWORD_SETTINGS as readonly string[]).includes(key)) values[`${key}_set`] = value !== "";
    else values[key] = value;
  }
  return values;
}

export async function saveSettings(values: Record<string, string>): Promise<void> {
  const known = await listSettings();
  for (const [key, value] of Object.entries(values)) {
    const isPassword = (PASSWORD_SETTINGS as readonly string[]).includes(key);
    if (!isPassword && !(key in known)) continue;
    if (isPassword && value === "") continue;
    await setSetting(key, isPassword ? seal(value) : value);
  }
}

/** Rewrites plaintext provider keys and password settings once CREDENTIALS_KEY is set. */
export async function migratePlaintextSecrets(): Promise<void> {
  if (!credentialsEnabled()) return;
  const providers = await pool.query<{ id: string; api_key: string }>(
    `SELECT id, api_key FROM llm_providers WHERE api_key <> '' AND api_key NOT LIKE 'enc1:%'`,
  );
  for (const row of providers.rows) {
    await pool.query(`UPDATE llm_providers SET api_key = $2 WHERE id = $1`, [row.id, encrypt(row.api_key)]);
  }
  const settings = await pool.query<{ key: string; value: string }>(
    `SELECT key, value FROM app_settings WHERE key = ANY($1) AND value <> '' AND value NOT LIKE 'enc1:%'`,
    [PASSWORD_SETTINGS],
  );
  for (const row of settings.rows) {
    await setSetting(row.key, encrypt(row.value));
  }
}
