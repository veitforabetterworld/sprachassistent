import type { Router } from "express";
import { z } from "zod";
import { getAuthUser } from "@veit/auth-server/express";
import { pool } from "./db.js";
import { handle, HttpError, parse, toVoice } from "./http.js";
import { finishOAuth, startOAuth } from "./oauth.js";
import { credentialsBox } from "./secrets.js";

/** MCP servers and built-in tool policies of the signed-in user. The voice server discovers and calls them. */

const uuid = z.string().uuid();

const PRESETS: Record<string, { url: string; auth_type: string }> = {
  homeassistant: { url: "", auth_type: "bearer" },
  calendar: { url: "https://calendarmcp.googleapis.com/mcp/v1", auth_type: "oauth" },
  gmail: { url: "https://gmailmcp.googleapis.com/mcp/v1", auth_type: "oauth" },
  custom: { url: "", auth_type: "none" },
};

const serverSchema = z.object({
  name: z.string().trim().min(1).max(80),
  preset: z.enum(["homeassistant", "calendar", "gmail", "custom"]).default("custom"),
  transport: z.enum(["http", "stdio"]).default("http"),
  url: z.string().trim().max(500).default(""),
  command: z.string().trim().max(300).default(""),
  enabled: z.boolean().default(true),
  auth_type: z.enum(["none", "bearer", "oauth"]).default("none"),
  /** Empty keeps the stored token. */
  secret: z.string().default(""),
});

const NATIVE = ["web_research"] as const;
const LEGACY_NATIVE = ["web_search", "fetch_url"] as const;

function userId(req: Parameters<typeof getAuthUser>[0]): string | number {
  const user = getAuthUser(req);
  if (!user) throw new HttpError(401, "auth.unauthenticated");
  return user.id;
}

export function registerMcp(router: Router): void {
  router.get(
    "/mcp/oauth/callback",
    handle(async (req, res) => {
      await finishOAuth(req, res);
    }),
  );

  router.get(
    "/mcp",
    handle(async (req, res) => {
      const id = userId(req);
      const servers = await pool.query(
        `SELECT id, name, preset, transport, url, command, enabled, auth_type,
                secret_enc <> '' AS secret_set, oauth_enc <> '' AS oauth_set,
                status, last_error, last_error_detail, last_ok_at
         FROM mcp_servers WHERE user_id = $1 ORDER BY name`,
        [id],
      );
      const tools = await pool.query(
        `SELECT t.server_id, t.name, t.description, t.enabled, t.effect, t.present
         FROM mcp_tools t JOIN mcp_servers s ON s.id = t.server_id
         WHERE s.user_id = $1 ORDER BY t.name`,
        [id],
      );
      res.json({ servers: servers.rows, tools: tools.rows });
    }),
  );

  router.post(
    "/mcp",
    handle(async (req, res) => {
      const id = userId(req);
      const body = parse(serverSchema, req.body);
      const preset = PRESETS[body.preset] ?? PRESETS.custom;
      const url = body.url || preset.url;
      const auth = body.preset === "custom" ? body.auth_type : preset.auth_type;
      const row = await pool.query(
        `INSERT INTO mcp_servers (user_id, name, preset, transport, url, command, enabled, auth_type, secret_enc)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING id, name, preset, transport, url, command, enabled, auth_type,
                   secret_enc <> '' AS secret_set, oauth_enc <> '' AS oauth_set, status, last_error, last_error_detail, last_ok_at`,
        [id, body.name, body.preset, body.transport, url, body.command, body.enabled, auth, credentialsBox.encrypt(body.secret)],
      );
      res.status(201).json(row.rows[0]);
    }),
  );

  router.put(
    "/mcp/:id",
    handle(async (req, res) => {
      const owner = userId(req);
      const id = parse(uuid, req.params.id);
      const body = parse(serverSchema, req.body);
      const row = await pool.query(
        `UPDATE mcp_servers SET name = $3, preset = $4, transport = $5, url = $6, command = $7, enabled = $8,
                auth_type = $9, secret_enc = CASE WHEN $10 = '' THEN secret_enc ELSE $10 END
         WHERE id = $1 AND user_id = $2
         RETURNING id, name, preset, transport, url, command, enabled, auth_type,
                   secret_enc <> '' AS secret_set, oauth_enc <> '' AS oauth_set, status, last_error, last_error_detail, last_ok_at`,
        [
          id,
          owner,
          body.name,
          body.preset,
          body.transport,
          body.url,
          body.command,
          body.enabled,
          body.auth_type,
          credentialsBox.encrypt(body.secret),
        ],
      );
      if (!row.rows[0]) throw new HttpError(404, "mcp.not_found");
      res.json(row.rows[0]);
    }),
  );

  router.delete(
    "/mcp/:id",
    handle(async (req, res) => {
      const result = await pool.query(`DELETE FROM mcp_servers WHERE id = $1 AND user_id = $2`, [
        parse(uuid, req.params.id),
        userId(req),
      ]);
      if (result.rowCount === 0) throw new HttpError(404, "mcp.not_found");
      res.status(204).end();
    }),
  );

  router.post(
    "/mcp/:id/discover",
    handle(async (req, res) => {
      const id = parse(uuid, req.params.id);
      const owned = await pool.query(`SELECT 1 FROM mcp_servers WHERE id = $1 AND user_id = $2`, [id, userId(req)]);
      if (!owned.rows[0]) throw new HttpError(404, "mcp.not_found");
      await toVoice(res, `/internal/mcp/${id}/discover`, { method: "POST", body: {} });
    }),
  );

  router.get(
    "/mcp/:id/oauth/start",
    handle(async (req, res) => {
      // The browser navigates here, so a failure goes back to the page instead of answering JSON.
      try {
        await startOAuth(req, res, userId(req), parse(uuid, req.params.id));
      } catch (error) {
        const code = error instanceof HttpError ? error.code : "internal";
        if (!(error instanceof HttpError)) console.error(error);
        res.redirect(`/werkzeuge?oauth=error&code=${encodeURIComponent(code)}`);
      }
    }),
  );

  router.put(
    "/mcp/:id/tools/:name",
    handle(async (req, res) => {
      const body = parse(z.object({ enabled: z.boolean(), effect: z.enum(["auto", "confirm", "deny"]) }), req.body);
      const row = await pool.query(
        `UPDATE mcp_tools t SET enabled = $4, effect = $5
         FROM mcp_servers s
         WHERE t.server_id = s.id AND s.id = $1 AND s.user_id = $2 AND t.name = $3
         RETURNING t.server_id, t.name, t.description, t.enabled, t.effect, t.present`,
        [parse(uuid, req.params.id), userId(req), req.params.name, body.enabled, body.effect],
      );
      if (!row.rows[0]) throw new HttpError(404, "tool.not_found");
      res.json(row.rows[0]);
    }),
  );

  router.get(
    "/tools/native",
    handle(async (req, res) => {
      const owner = userId(req);
      const rows = await pool.query<{ name: string; effect: string; enabled: boolean }>(
        `SELECT name, effect, enabled FROM native_tool_policies WHERE user_id = $1 AND name = ANY($2)`,
        [owner, [...NATIVE, ...LEGACY_NATIVE]],
      );
      const byName = new Map(rows.rows.map((row) => [row.name, row]));
      const own = byName.get("web_research");
      const legacyBlocked = LEGACY_NATIVE.some((name) => {
        const row = byName.get(name);
        return row != null && (!row.enabled || row.effect === "deny");
      });
      const tool = own ??
        (legacyBlocked
          ? { name: "web_research", effect: "deny", enabled: false }
          : { name: "web_research", effect: "auto", enabled: true });
      res.json({ tools: [tool] });
    }),
  );

  router.put(
    "/tools/native/:name",
    handle(async (req, res) => {
      const name = parse(z.enum(NATIVE), req.params.name);
      const body = parse(z.object({ enabled: z.boolean(), effect: z.enum(["auto", "confirm", "deny"]) }), req.body);
      const row = await pool.query(
        `INSERT INTO native_tool_policies (user_id, name, effect, enabled) VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, name) DO UPDATE SET effect = EXCLUDED.effect, enabled = EXCLUDED.enabled
         RETURNING name, effect, enabled`,
        [userId(req), name, body.effect, body.enabled],
      );
      res.json(row.rows[0]);
    }),
  );
}
