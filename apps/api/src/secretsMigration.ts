import type { PoolClient } from "pg";
import { reencryptApiKeys } from "@veit/api-keys/migrate";
import { createSecretBox } from "@veit/secret-box";
import { ReencryptError, decodeLegacy, reencryptRows, type LegacySource, type ReencryptResult, type SqlQuery } from "@veit/secret-box/migrate";
import { CREDENTIALS_CONTEXT, MFA_TOTP_CONTEXT, credentialsBox } from "./secrets.js";
import { ADMIN_SECRETS, INTERNAL_SECRETS } from "./settings.js";

/**
 * One-time data migration onto @veit/secret-box (`enc1:`), run by migrate.ts inside its transaction and recorded
 * in schema_migrations under SECRETS_MIGRATION. Afterwards no stored secret is plain text or in an older layout:
 *
 * - app_settings: the generated keys (auth_mfa_key, api_key_pepper, api_key_secret) and the admin passwords
 *   (smtp_password, google_oauth_client_secret), plain text before, now with CREDENTIALS_KEY;
 * - llm_providers.api_key, mcp_servers.secret_enc and oauth_enc: plain text where CREDENTIALS_KEY was missing
 *   when they were stored; values already in `enc1:` stay as they are (the voice server reads the same layout);
 * - users.mfa_totp_secret_enc and mfa_totp_setup_secret_enc: the old layout of @veit/auth-server
 *   (createAesGcmSecretBox, base64 JSON without prefix), key material auth_mfa_key;
 * - user_api_keys.key_enc: the old encryptApiKey layout (`iv.tag.data`), key material api_key_secret.
 *
 * The values themselves do not change, only how they are stored: keys, hashes and TOTP codes keep working.
 */

export const SECRETS_MIGRATION = "data_0001_secret_box";

const PLAINTEXT: LegacySource[] = [{ kind: "plaintext" }];

/** reencryptRows wraps its work in BEGIN/COMMIT; inside migrate's transaction that becomes a savepoint. */
function nested(client: PoolClient): SqlQuery {
  return async (text, params) => {
    if (text === "BEGIN") return client.query("SAVEPOINT secrets_migration");
    if (text === "COMMIT") return client.query("RELEASE SAVEPOINT secrets_migration");
    if (text === "ROLLBACK") return client.query("ROLLBACK TO SAVEPOINT secrets_migration");
    return client.query(text, params);
  };
}

/** A generated key from app_settings, plain text or already `enc1:`; '' when there is none yet. */
async function internalKey(client: PoolClient, key: string): Promise<string> {
  const row = await client.query<{ value: string }>(`SELECT value FROM app_settings WHERE key = $1`, [key]);
  const stored = row.rows[0]?.value ?? "";
  return decodeLegacy(stored, [
    { kind: "canonical", key: process.env.CREDENTIALS_KEY ?? "", context: CREDENTIALS_CONTEXT },
    ...PLAINTEXT,
  ]);
}

async function count(client: PoolClient, sql: string): Promise<number> {
  return Number((await client.query<{ n: string }>(sql)).rows[0]?.n ?? 0);
}

/** Converts every stored secret; throws (and the caller rolls back) when one cannot be read. */
export async function migrateSecrets(client: PoolClient): Promise<void> {
  if (!credentialsBox.enabled()) {
    throw new Error("CREDENTIALS_KEY fehlt: ohne Schlüssel lassen sich die Geheimnisse nicht umstellen (.env)");
  }
  const query = nested(client);
  const log: string[] = [];
  const note = async (what: string, run: Promise<ReencryptResult>) => {
    try {
      const result = await run;
      if (result.converted > 0) log.push(`${what}: ${result.converted}`);
    } catch (error) {
      if (!(error instanceof ReencryptError)) throw error;
      const rows = error.result.failed.map((f) => `${f.id}: ${f.error}`).join("; ");
      throw new Error(`${what} nicht lesbar, nichts umgestellt (${rows})`, { cause: error });
    }
  };

  // Read the key material before app_settings itself is converted.
  const mfaKey = await internalKey(client, "auth_mfa_key");
  const keySecret = await internalKey(client, "api_key_secret");

  const totp = ["mfa_totp_secret_enc", "mfa_totp_setup_secret_enc"] as const;
  if (mfaKey) {
    const box = createSecretBox({ key: mfaKey, context: MFA_TOTP_CONTEXT });
    for (const column of totp) {
      await note(
        `users.${column}`,
        reencryptRows({
          client: query,
          table: "users",
          idColumn: "id",
          column,
          box,
          sources: [{ kind: "auth-json", key: mfaKey, context: MFA_TOTP_CONTEXT }],
        }),
      );
    }
  } else if (
    (await count(client, `SELECT count(*) AS n FROM users WHERE COALESCE(${totp[0]}, '') <> '' OR COALESCE(${totp[1]}, '') <> ''`)) > 0
  ) {
    throw new Error("auth_mfa_key fehlt in app_settings, TOTP-Secrets lassen sich nicht umstellen");
  }

  if (keySecret) {
    await note("user_api_keys.key_enc", reencryptApiKeys({ client: query, oldSecret: keySecret }));
  } else if ((await count(client, `SELECT count(*) AS n FROM user_api_keys WHERE COALESCE(key_enc, '') <> ''`)) > 0) {
    throw new Error("api_key_secret fehlt in app_settings, gespeicherte API-Keys lassen sich nicht umstellen");
  }

  const columns = [
    { table: "llm_providers", idColumn: "id", column: "api_key" },
    { table: "mcp_servers", idColumn: "id", column: "secret_enc" },
    { table: "mcp_servers", idColumn: "id", column: "oauth_enc" },
  ];
  for (const c of columns) {
    await note(`${c.table}.${c.column}`, reencryptRows({ client: query, ...c, box: credentialsBox, sources: PLAINTEXT }));
  }
  await note(
    "app_settings",
    reencryptRows({
      client: query,
      table: "app_settings",
      idColumn: "key",
      column: "value",
      box: credentialsBox,
      sources: PLAINTEXT,
      where: "key = ANY($1)",
      params: [[...INTERNAL_SECRETS, ...ADMIN_SECRETS]],
    }),
  );
  console.log(log.length ? `Geheimnisse umgestellt (${log.join(", ")})` : "Keine Geheimnisse umzustellen.");
}
