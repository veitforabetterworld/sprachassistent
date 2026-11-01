import { randomBytes } from "node:crypto";
import { pool } from "./db.js";

/** Generated once, used by the server only and never sent to the browser. */
const SECRET_KEYS = ["api_key_pepper", "auth_mfa_key"] as const;
/** Write-only in the admin area: GET says whether they are set, an empty value on PUT keeps them. */
export const PASSWORD_SETTINGS = ["smtp_password"] as const;

export async function getSetting(key: string): Promise<string | null> {
  const result = await pool.query<{ value: string }>(`SELECT value FROM app_settings WHERE key = $1`, [key]);
  return result.rows[0]?.value ?? null;
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
    await setSetting(key, value);
  }
}
