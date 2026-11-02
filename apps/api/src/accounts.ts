import type { Request } from "express";
import { AuthService, authFail, createAesGcmSecretBox, type UserId } from "@veit/auth-server";
import { createExpressAuth, getAuthUser } from "@veit/auth-server/express";
import { createPgAuthStore } from "@veit/auth-server/pg";
import { createSmtpMailer, germanMailTexts } from "@veit/auth-server/smtp";
import { ApiKeyError, ApiKeyService, type ApiKeyInput } from "@veit/api-keys";
import { createExpressApiKeys } from "@veit/api-keys/express";
import { createPgApiKeyStore } from "@veit/api-keys/pg";
import { pool } from "./db.js";
import { ensureSecret, getSetting } from "./settings.js";

/**
 * Accounts (veit-framework: @veit/auth-server, @veit/api-keys).
 *
 * Admins sign in with a session cookie. An API key belongs to one chatbot of its owner. The stored
 * permission is fixed; the voice server checks the key against user_api_keys.
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

/** A key always carries the fixed permission and exactly one chatbot. */
function oneChatbot(resources: string[] | undefined): string[] {
  const ids = [...new Set((resources ?? []).filter(Boolean))];
  if (ids.length !== 1) throw new ApiKeyError("api_keys.one_device", 400);
  return ids;
}

class ChatbotApiKeyService extends ApiKeyService {
  override create(userId: UserId, input: ApiKeyInput) {
    return super.create(userId, {
      ...input,
      permissions: [DEVICE_TALK],
      allResources: false,
      resources: oneChatbot(input.resources),
    });
  }

  override update(userId: UserId, id: string, patch: Partial<ApiKeyInput>) {
    const next: Partial<ApiKeyInput> = { ...patch, permissions: [DEVICE_TALK] };
    if (patch.allResources || patch.resources !== undefined) {
      next.allResources = false;
      next.resources = oneChatbot(patch.resources);
    }
    return super.update(userId, id, next);
  }
}

export const apiKeyService = new ChatbotApiKeyService({
  store: createPgApiKeyStore(pool),
  permissions: [DEVICE_TALK],
  prefix: KEY_PREFIX,
  pepper: () => pepper,
  // Keys stay revealable, so they can be copied into a chatbot again later.
  secret: () => keySecret,
  // A key may only name a chatbot of its own user.
  foreignResources: async (userId, resources) => {
    if (resources.length === 0) return [];
    const known = await pool.query<{ id: string }>(
      `SELECT id::text FROM devices WHERE user_id = $1 AND id::text = ANY($2)`,
      [userId, resources],
    );
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
