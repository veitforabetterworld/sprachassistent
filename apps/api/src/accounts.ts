import type { Request } from "express";
import { AuthService, authFail, createAesGcmSecretBox, isAuthError, type AuthMailer, type RequestInfo, type UserId } from "@veit/auth-server";
import { createExpressAuth, getAuthUser } from "@veit/auth-server/express";
import { createPgAuthStore } from "@veit/auth-server/pg";
import { createSmtpMailer, germanMailTexts } from "@veit/auth-server/smtp";
import { ApiKeyError, ApiKeyService, type ApiKeyInput } from "@veit/api-keys";
import { createExpressApiKeys } from "@veit/api-keys/express";
import { createPgApiKeyStore } from "@veit/api-keys/pg";
import { pool } from "./db.js";
import { HttpError } from "./http.js";
import { ensureSecret, getSetting, setSetting } from "./settings.js";

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

/** On: a new account must confirm its address before it can use the app. Off when no SMTP is set up. */
export async function verificationRequired(): Promise<boolean> {
  return (await getSetting("account_verification")) === "1";
}

export async function setVerificationRequired(on: boolean): Promise<void> {
  await setSetting("account_verification", on ? "1" : "0");
}

const smtpMailer = createSmtpMailer(smtpSettings, germanMailTexts("Sprachassistent"));
const mailer: AuthMailer = {
  ...smtpMailer,
  sendVerification: async (input) => {
    if (!(await verificationRequired())) return;
    await smtpMailer.sendVerification(input);
  },
};

export const authService = new AuthService({
  store: createPgAuthStore(pool),
  mailer,
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
    if (await verificationRequired()) return;
    await pool.query(
      `UPDATE users SET email_verified_at = NOW(), email_verification_token = NULL, email_verification_expires_at = NULL WHERE id = $1`,
      [userId],
    );
  },
  beforeDeleteUser: async (userId) => {
    if (await isAdmin(userId)) {
      const others = await pool.query(`SELECT 1 FROM users WHERE is_admin AND id <> $1 LIMIT 1`, [userId]);
      if (others.rowCount === 0) authFail("auth.last_admin", 400);
    }
    await pool.query(`DELETE FROM native_tool_policies WHERE user_id = $1`, [userId]);
    await pool.query(`DELETE FROM oauth_states WHERE user_id = $1`, [userId]);
  },
});

export const auth = createExpressAuth({
  service: authService,
  cookieName: "spa_session",
  extendUser: async (user) => ({
    is_admin: await isAdmin(user.id),
    verification_required: await verificationRequired(),
  }),
});

export type AccountRow = {
  id: string;
  email: string;
  first_name: string;
  last_name: string;
  email_verified_at: string | null;
  is_admin: boolean;
  created_at: string;
};

const ACCOUNT_COLUMNS = `id::text, email, first_name, last_name, email_verified_at, is_admin, created_at`;

export async function listAccounts(): Promise<AccountRow[]> {
  const result = await pool.query<AccountRow>(`SELECT ${ACCOUNT_COLUMNS} FROM users ORDER BY id`);
  return result.rows;
}

export async function approveAccount(id: string): Promise<AccountRow> {
  const result = await pool.query<AccountRow>(
    `UPDATE users SET email_verified_at = COALESCE(email_verified_at, NOW()),
       email_verification_token = NULL, email_verification_expires_at = NULL
     WHERE id = $1 RETURNING ${ACCOUNT_COLUMNS}`,
    [id],
  );
  if (!result.rows[0]) throw new HttpError(404, "Konto nicht gefunden");
  return result.rows[0];
}

export async function removeAccount(actorId: UserId, targetId: string): Promise<void> {
  if (String(actorId) === targetId) throw new HttpError(400, "Das eigene Konto löschst du unter Konto.");
  const exists = await pool.query(`SELECT 1 FROM users WHERE id = $1`, [targetId]);
  if (!exists.rows[0]) throw new HttpError(404, "Konto nicht gefunden");
  try {
    await authService.deleteAccount(targetId);
  } catch (error) {
    if (isAuthError(error) && error.code === "auth.last_admin") {
      throw new HttpError(400, "Das letzte Adminkonto bleibt.");
    }
    throw error;
  }
}

function requestInfo(req: Request): RequestInfo {
  const forwarded = String(req.headers["x-forwarded-proto"] ?? "").split(",")[0].trim();
  const proto = req.secure || forwarded === "https" ? "https" : "http";
  const host = String(req.headers["x-forwarded-host"] ?? req.headers.host ?? "localhost").split(",")[0].trim();
  return {
    ip: req.ip ?? null,
    userAgent: req.get("user-agent") ?? null,
    origin: `${proto}://${host}`,
    deviceName: null,
  };
}

export async function sendResetLink(targetId: string, req: Request): Promise<void> {
  const row = await pool.query<{ email: string }>(`SELECT email FROM users WHERE id = $1`, [targetId]);
  if (!row.rows[0]) throw new HttpError(404, "Konto nicht gefunden");
  try {
    await authService.requestPasswordReset(row.rows[0].email, requestInfo(req));
  } catch (error) {
    if (!isAuthError(error)) throw error;
    if (error.code === "auth.mail_unavailable") throw new HttpError(503, "E-Mail ist nicht eingerichtet.");
    if (error.code === "auth.password_reset_too_many_requests") throw new HttpError(429, "Zu viele Links für diese Adresse.");
    throw new HttpError(error.status, error.code);
  }
}

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
