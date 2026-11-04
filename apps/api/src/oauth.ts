import { randomBytes } from "node:crypto";
import type { Request, Response } from "express";
import type { UserId } from "@veit/auth-server";
import { getAuthUser } from "@veit/auth-server/express";
import { pool } from "./db.js";
import { HttpError } from "./http.js";
import { credentialsBox } from "./secrets.js";
import { appSettings } from "./settings.js";

/** Browser OAuth for the official Google MCP servers. The voice server only refreshes the stored token. */

const AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL = "https://oauth2.googleapis.com/token";

const SCOPES: Record<string, string> = {
  calendar: "https://www.googleapis.com/auth/calendar",
  gmail: "https://www.googleapis.com/auth/gmail.modify",
};

function originOf(req: Request): string {
  return `${req.protocol}://${req.get("host")}`;
}

function redirectUri(req: Request): string {
  return `${originOf(req)}/v1/admin/mcp/oauth/callback`;
}

export async function startOAuth(req: Request, res: Response, userId: UserId, serverId: string): Promise<void> {
  const row = await pool.query<{ preset: string; user_id: string }>(
    `SELECT preset, user_id::text FROM mcp_servers WHERE id = $1 AND user_id = $2`,
    [serverId, userId],
  );
  const server = row.rows[0];
  if (!server) throw new HttpError(404, "mcp.not_found");
  const scope = SCOPES[server.preset];
  if (!scope) throw new HttpError(400, "mcp.not_oauth");
  const clientId = (await appSettings.get("google_oauth_client_id")) ?? "";
  if (!clientId || !(await appSettings.get("google_oauth_client_secret"))) {
    throw new HttpError(400, "mcp.oauth_not_configured");
  }
  const state = randomBytes(24).toString("base64url");
  await pool.query(`DELETE FROM oauth_states WHERE expires_at < NOW()`);
  await pool.query(
    `INSERT INTO oauth_states (state, user_id, server_id, expires_at) VALUES ($1, $2, $3, NOW() + INTERVAL '10 minutes')`,
    [state, userId, serverId],
  );
  const url = new URL(AUTH_URL);
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("redirect_uri", redirectUri(req));
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", scope);
  url.searchParams.set("access_type", "offline");
  url.searchParams.set("prompt", "consent");
  url.searchParams.set("state", state);
  res.redirect(url.toString());
}

export async function finishOAuth(req: Request, res: Response): Promise<void> {
  const back = (query: string) => res.redirect(`/werkzeuge?${query}`);
  const user = getAuthUser(req);
  const state = String(req.query.state ?? "");
  const code = String(req.query.code ?? "");
  if (!user || !state || !code) {
    back("oauth=error");
    return;
  }
  const saved = await pool.query<{ user_id: string; server_id: string }>(
    `DELETE FROM oauth_states WHERE state = $1 AND expires_at > NOW() RETURNING user_id::text, server_id::text`,
    [state],
  );
  const row = saved.rows[0];
  if (!row || String(row.user_id) !== String(user.id)) {
    back("oauth=error");
    return;
  }
  const clientId = (await appSettings.get("google_oauth_client_id")) ?? "";
  const clientSecret = (await appSettings.get("google_oauth_client_secret")) ?? "";
  const body = new URLSearchParams({
    grant_type: "authorization_code",
    code,
    client_id: clientId,
    client_secret: clientSecret,
    redirect_uri: redirectUri(req),
  });
  const token = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body,
  });
  if (!token.ok) {
    back("oauth=error");
    return;
  }
  const json = (await token.json()) as { access_token?: string; refresh_token?: string; expires_in?: number };
  if (!json.refresh_token || !json.access_token) {
    back("oauth=error");
    return;
  }
  const blob = JSON.stringify({
    refresh_token: json.refresh_token,
    access_token: json.access_token,
    expires_at: Math.floor(Date.now() / 1000) + (json.expires_in ?? 3600),
  });
  await pool.query(`UPDATE mcp_servers SET oauth_enc = $2, auth_type = 'oauth', status = 'unknown', last_error = '', last_error_detail = '' WHERE id = $1 AND user_id = $3`, [
    row.server_id,
    credentialsBox.encrypt(blob),
    user.id,
  ]);
  back("oauth=ok");
}
