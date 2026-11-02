import type { Request } from "express";
import { AuthService, authFail, createAesGcmSecretBox, type UserId } from "@veit/auth-server";
import { createExpressAuth, getAuthUser } from "@veit/auth-server/express";
import { createPgAuthStore } from "@veit/auth-server/pg";
import { createSmtpMailer, germanMailTexts } from "@veit/auth-server/smtp";
import { ApiKeyService } from "@veit/api-keys";
import { createExpressApiKeys } from "@veit/api-keys/express";
import { createPgApiKeyStore } from "@veit/api-keys/pg";
import { pool } from "./db.js";
import { ensureSecret, getSetting } from "./settings.js";

/**
 * Accounts (veit-framework: @veit/auth-server, @veit/api-keys).
 *
 * Admins sign in with a session cookie. ESP keys are API keys of an admin with the permission below and the
 * device ids as resources; the Rust voice server checks them against user_api_keys.
 */

export const DEVICE_TALK = "device.talk";
/** Key prefix, also checked by the voice server. */
export const KEY_PREFIX = "spa";

let mfaKey = "";
let pepper = "";
let keySecret = "";

export async function ensureAccountSecrets(): Promise<void> {
  mfaKey = await ensureSecret("auth_mfa_key");
  pepper = await ensureSecret("api_key_pepper");
  keySecret = await ensureSecret("api_key_secret");
}

async function smtpSettings() {
  const host = (await getSetting("smtp_host")) ?? "";
  if (!host) return null;
  return {
    host,
    port: Number((await getSetting("smtp_port")) ?? "587") || 587,
    secure: (await getSetting("smtp_secure")) === "1",
    user: (await getSetting("smtp_user")) ?? "",
    password: (await getSetting("smtp_password")) ?? "",
    from: (await getSetting("smtp_from")) ?? "",
  };
}

export async function isAdmin(userId: UserId): Promise<boolean> {
  const result = await pool.query<{ is_admin: boolean }>(`SELECT is_admin FROM users WHERE id = $1`, [userId]);
  return result.rows[0]?.is_admin ?? false;
}

export async function adminExists(): Promise<boolean> {
  return ((await pool.query(`SELECT 1 FROM users WHERE is_admin LIMIT 1`)).rowCount ?? 0) > 0;
}

export const authService = new AuthService({
  store: createPgAuthStore(pool),
  mailer: createSmtpMailer(smtpSettings, germanMailTexts("Sprachassistent")),
  secretBox: createAesGcmSecretBox(() => mfaKey, "sprachassistent-mfa-totp"),
  totpIssuer: "Sprachassistent",
  // Pages of apps/web.
  verifyEmailUrl: (token, origin) => `${origin}/e-mail-bestaetigen?token=${encodeURIComponent(token)}`,
  resetPasswordUrl: (token, origin) => `${origin}/passwort-neu?token=${encodeURIComponent(token)}`,
  // First setup: the first account becomes admin without waiting for a mail (SMTP is set in the admin area).
  afterRegister: async (userId) => {
    await pool.query(
      `UPDATE users SET is_admin = true WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM users WHERE is_admin)`,
      [userId],
    );
  },
  beforeDeleteUser: async (userId) => {
    if (!(await isAdmin(userId))) return;
    const others = await pool.query(`SELECT 1 FROM users WHERE is_admin AND id <> $1 LIMIT 1`, [userId]);
    if (others.rowCount === 0) authFail("auth.last_admin", 400);
  },
});

export const auth = createExpressAuth({
  service: authService,
  cookieName: "spa_session",
  extendUser: async (user) => ({ is_admin: await isAdmin(user.id) }),
});

export const apiKeyService = new ApiKeyService({
  store: createPgApiKeyStore(pool),
  permissions: [DEVICE_TALK],
  prefix: KEY_PREFIX,
  pepper: () => pepper,
  // ESP keys stay revealable, so they can be copied into a device again later.
  secret: () => keySecret,
  // Only admins may hand out keys, and only for existing devices.
  foreignResources: async (userId, resources) => {
    if (!(await isAdmin(userId))) return resources;
    if (resources.length === 0) return [];
    const known = await pool.query<{ id: string }>(`SELECT id::text FROM devices WHERE id::text = ANY($1)`, [resources]);
    const ids = new Set(known.rows.map((row) => row.id));
    return resources.filter((resource) => !ids.has(resource));
  },
});

export const apiKeys = createExpressApiKeys({
  service: apiKeyService,
  userId: (req) => getAuthUser(req)?.id ?? null,
});

export async function adminDecision(req: Request) {
  const user = getAuthUser(req);
  if (!user) return { ok: false as const, status: 401, error: "Bitte anmelden" };
  if (!(await isAdmin(user.id))) return { ok: false as const, status: 403, error: "Kein Adminkonto" };
  return { ok: true as const };
}
