import { createAppSettings } from "@veit/app-settings";
import { DEFAULT_SMTP_SETTING_KEYS } from "@veit/app-settings/smtp";
import { pool } from "./db.js";
import { credentialsBox } from "./secrets.js";

/** Generated once, used by the servers only and never sent to the browser. The voice server reads the pepper. */
export const INTERNAL_SECRETS = ["api_key_pepper", "api_key_secret", "auth_mfa_key"] as const;
/** Shown masked in the admin area; an admin can reveal them, an empty value on save keeps them. */
export const ADMIN_SECRETS = ["smtp_password", "google_oauth_client_secret"] as const;

/** What the admin area lists and may change. account_verification has its own route on the users page. */
export const ADMIN_SETTING_KEYS = [
  ...Object.values(DEFAULT_SMTP_SETTING_KEYS),
  "google_oauth_client_id",
  "google_oauth_client_secret",
  "stt_model",
  "stt_threads",
  "tts_threads",
] as const;

/** app_settings (@veit/app-settings); secrets in the format of the voice server's credentials. */
export const appSettings = createAppSettings({
  pool,
  secretBox: credentialsBox,
  secretKeys: [...INTERNAL_SECRETS, ...ADMIN_SECRETS],
});
