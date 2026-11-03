import express, { Router, type NextFunction, type Request, type Response } from "express";
import { z } from "zod";
import { getAuthUser } from "@veit/auth-server/express";
import {
  approveAccount,
  isAdmin,
  listAccounts,
  removeAccount,
  sendResetLink,
  setVerificationRequired,
  verificationRequired,
} from "./accounts.js";
import { pool } from "./db.js";
import { handle, HttpError, parse, toVoice, toVoiceBuffer } from "./http.js";
import { registerMcp } from "./mcp.js";
import { openSecret, seal } from "./secrets.js";
import { PASSWORD_SETTINGS, getSetting, listSettings, saveSettings } from "./settings.js";

/** Signed-in area. Settings are admin-only. Providers, devices and tools belong to the session user. */

const uuid = z.string().uuid();

async function ownerId(req: Request): Promise<string | number> {
  const user = getAuthUser(req);
  if (!user) throw new HttpError(401, "Bitte anmelden");
  return user.id;
}

/** 404 for a device that belongs to someone else, so one user cannot read another's memory. */
async function ownedDevice(req: Request, id: string): Promise<string> {
  const row = await pool.query(`SELECT 1 FROM devices WHERE id = $1 AND user_id = $2`, [id, await ownerId(req)]);
  if (!row.rows[0]) throw new HttpError(404, "Gerät nicht gefunden");
  return id;
}

/** 404 for a provider that belongs to someone else. Null stays null (the device then uses its owner's list). */
async function ownedProvider(req: Request, id: string | null): Promise<string | null> {
  if (!id) return null;
  const row = await pool.query(`SELECT 1 FROM llm_providers WHERE id = $1 AND user_id = $2`, [id, await ownerId(req)]);
  if (!row.rows[0]) throw new HttpError(404, "Modell nicht gefunden");
  return id;
}

const providerSchema = z.object({
  name: z.string().trim().min(1).max(80),
  base_url: z.string().trim().url(),
  /** Empty keeps the stored key. */
  api_key: z.string().trim().default(""),
  model: z.string().trim().min(1).max(120),
  enabled: z.boolean().default(true),
  priority: z.coerce.number().int().default(0),
  nudge_tools: z.boolean().default(false),
});

const providerDraftSchema = providerSchema.pick({ name: true, base_url: true, api_key: true, model: true }).extend({
  id: uuid.nullable().default(null),
});

const deviceSchema = z.object({
  name: z.string().trim().min(1).max(80),
  voice: z.string().trim().min(1),
  speed: z.coerce.number().min(0.5).max(2).default(1),
  persona: z.string().max(8000).default(""),
  provider_id: uuid.nullable().default(null),
  max_iterations: z.coerce.number().int().min(1).max(20).default(6),
  busy_phrase: z.string().trim().min(1).max(200).default("Einen Moment."),
  still_busy_phrase: z.string().trim().min(1).max(200).default("Ich bin noch dran."),
  tools_enabled: z.boolean().default(true),
});

const DEVICE_COLUMNS = `id, name, voice, speed, persona, provider_id, max_iterations, busy_phrase,
  still_busy_phrase, tools_enabled, created_at`;

/** Same limit as CONTEXT_CHARS in src/context.rs. */
const CONTEXT_CHARS = 500;

function keyHint(key: string): string {
  return key.length > 8 ? `…${key.slice(-4)}` : "";
}

function publicProvider(row: { api_key: string; id: string; name: string; base_url: string; model: string; enabled: boolean; priority: number; nudge_tools: boolean }) {
  const key = openSecret(row.api_key);
  return {
    id: row.id,
    name: row.name,
    base_url: row.base_url,
    model: row.model,
    enabled: row.enabled,
    priority: row.priority,
    nudge_tools: row.nudge_tools,
    api_key_set: row.api_key !== "",
    api_key_hint: keyHint(key),
  };
}

export function adminRouter(): Router {
  const router = Router();

  router.use((req, res, next) => {
    const user = getAuthUser(req);
    if (!user) {
      res.status(401).json({ error: "Bitte anmelden" });
      return;
    }
    const adminOnly = req.path.startsWith("/settings") || req.path.startsWith("/users");
    if (!adminOnly) {
      next();
      return;
    }
    isAdmin(user.id)
      .then((ok) => (ok ? next() : res.status(403).json({ error: "Kein Adminkonto" })))
      .catch(next);
  });

  router.get("/status", handle((_req, res) => toVoice(res, "/v1/health")));
  router.get("/voices", handle((_req, res) => toVoice(res, "/internal/voices")));

  const userIdParam = z.string().regex(/^\d+$/);

  router.get(
    "/users",
    handle(async (_req, res) => {
      res.json({ users: await listAccounts(), verification_required: await verificationRequired() });
    }),
  );
  router.put(
    "/users/verification",
    handle(async (req, res) => {
      const body = parse(z.object({ enabled: z.boolean() }), req.body);
      await setVerificationRequired(body.enabled);
      res.json({ verification_required: await verificationRequired() });
    }),
  );
  router.post(
    "/users/:id/approve",
    handle(async (req, res) => {
      res.json(await approveAccount(parse(userIdParam, req.params.id)));
    }),
  );
  router.post(
    "/users/:id/password-reset",
    handle(async (req, res) => {
      await sendResetLink(parse(userIdParam, req.params.id), req);
      res.json({ ok: true });
    }),
  );
  router.delete(
    "/users/:id",
    handle(async (req, res) => {
      const actor = getAuthUser(req);
      if (!actor) throw new HttpError(401, "Bitte anmelden");
      await removeAccount(actor.id, parse(userIdParam, req.params.id));
      res.status(204).end();
    }),
  );

  router.get(
    "/settings",
    handle(async (_req, res) => res.json({ values: await listSettings() })),
  );
  router.put(
    "/settings",
    handle(async (req, res) => {
      await saveSettings(parse(z.record(z.string(), z.string()), req.body?.values));
      res.json({ values: await listSettings() });
    }),
  );
  // A stored password, to show and copy it in the edit dialog.
  router.get(
    "/settings/:key/secret",
    handle(async (req, res) => {
      const key = parse(z.enum(PASSWORD_SETTINGS), req.params.key);
      res.set("Cache-Control", "no-store").json({ value: (await getSetting(key)) ?? "" });
    }),
  );

  // ---------------------------------------------------------------- AI providers

  router.get(
    "/providers",
    handle(async (req, res) => {
      const rows = await pool.query(
        `SELECT id, name, base_url, model, enabled, priority, nudge_tools, api_key
         FROM llm_providers WHERE user_id = $1 ORDER BY priority, name`,
        [await ownerId(req)],
      );
      res.json({ providers: rows.rows.map(publicProvider) });
    }),
  );
  router.post(
    "/providers",
    handle(async (req, res) => {
      const p = parse(providerSchema, req.body);
      const row = await pool.query(
        `INSERT INTO llm_providers (user_id, name, base_url, api_key, model, enabled, priority, nudge_tools)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, name, base_url, model, enabled, priority, nudge_tools, api_key`,
        [await ownerId(req), p.name, p.base_url, p.api_key ? seal(p.api_key) : "", p.model, p.enabled, p.priority, p.nudge_tools],
      );
      res.status(201).json(publicProvider(row.rows[0]));
    }),
  );
  router.put(
    "/providers/:id",
    handle(async (req, res) => {
      const id = await ownedProvider(req, parse(uuid, req.params.id));
      const p = parse(providerSchema, req.body);
      const row = await pool.query(
        `UPDATE llm_providers SET name = $2, base_url = $3,
           api_key = CASE WHEN $4 = '' THEN api_key ELSE $4 END,
           model = $5, enabled = $6, priority = $7, nudge_tools = $8
         WHERE id = $1 AND user_id = $9
         RETURNING id, name, base_url, model, enabled, priority, nudge_tools, api_key`,
        [id, p.name, p.base_url, p.api_key ? seal(p.api_key) : "", p.model, p.enabled, p.priority, p.nudge_tools, await ownerId(req)],
      );
      if (!row.rows[0]) throw new HttpError(404, "Modell nicht gefunden");
      res.json(publicProvider(row.rows[0]));
    }),
  );
  router.delete(
    "/providers/:id",
    handle(async (req, res) => {
      const result = await pool.query(`DELETE FROM llm_providers WHERE id = $1 AND user_id = $2`, [
        await ownedProvider(req, parse(uuid, req.params.id)),
        await ownerId(req),
      ]);
      if (result.rowCount === 0) throw new HttpError(404, "Modell nicht gefunden");
      res.status(204).end();
    }),
  );
  // The stored key, to show and copy it in the edit dialog.
  router.get(
    "/providers/:id/key",
    handle(async (req, res) => {
      const row = await pool.query<{ api_key: string }>(
        `SELECT api_key FROM llm_providers WHERE id = $1 AND user_id = $2`,
        [await ownedProvider(req, parse(uuid, req.params.id)), await ownerId(req)],
      );
      if (!row.rows[0]) throw new HttpError(404, "Modell nicht gefunden");
      const key = openSecret(row.rows[0].api_key);
      if (row.rows[0].api_key.startsWith("enc1:") && !key) throw new HttpError(500, "CREDENTIALS_KEY fehlt");
      res.set("Cache-Control", "no-store").json({ key });
    }),
  );
  // Tests what the edit dialog shows, saved or not. An empty key uses the one stored under id.
  for (const kind of ["test", "tool-test"]) {
    router.post(
      `/providers/${kind}`,
      handle(async (req, res) => {
        const draft = parse(providerDraftSchema, req.body);
        const id = await ownedProvider(req, draft.id);
        await toVoice(res, `/internal/providers/${kind}`, {
          method: "POST",
          body: { id, name: draft.name, base_url: draft.base_url, model: draft.model, api_key: draft.api_key },
        });
      }),
    );
  }

  router.get(
    "/provider-choices",
    handle(async (req, res) => {
      const rows = await pool.query(`SELECT id, name FROM llm_providers WHERE user_id = $1 ORDER BY priority, name`, [
        await ownerId(req),
      ]);
      res.json({ providers: rows.rows });
    }),
  );

  // ---------------------------------------------------------------- devices

  router.get(
    "/devices",
    handle(async (req, res) => {
      const rows = await pool.query(`SELECT ${DEVICE_COLUMNS} FROM devices WHERE user_id = $1 ORDER BY created_at`, [
        await ownerId(req),
      ]);
      res.json({ devices: rows.rows });
    }),
  );
  router.post(
    "/devices",
    handle(async (req, res) => {
      const d = parse(deviceSchema, req.body);
      const providerId = await ownedProvider(req, d.provider_id);
      const row = await pool.query(
        `INSERT INTO devices (user_id, name, voice, speed, persona, provider_id,
            max_iterations, busy_phrase, still_busy_phrase, tools_enabled)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING ${DEVICE_COLUMNS}`,
        [
          await ownerId(req),
          d.name,
          d.voice,
          d.speed,
          d.persona,
          providerId,
          d.max_iterations,
          d.busy_phrase,
          d.still_busy_phrase,
          d.tools_enabled,
        ],
      );
      res.status(201).json(row.rows[0]);
    }),
  );
  router.put(
    "/devices/:id",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      const d = parse(deviceSchema, req.body);
      const providerId = await ownedProvider(req, d.provider_id);
      const row = await pool.query(
        `UPDATE devices SET name = $2, voice = $3, speed = $4, persona = $5, provider_id = $6,
           max_iterations = $7, busy_phrase = $8, still_busy_phrase = $9, tools_enabled = $10
         WHERE id = $1 AND user_id = $11 RETURNING ${DEVICE_COLUMNS}`,
        [
          id,
          d.name,
          d.voice,
          d.speed,
          d.persona,
          providerId,
          d.max_iterations,
          d.busy_phrase,
          d.still_busy_phrase,
          d.tools_enabled,
          await ownerId(req),
        ],
      );
      if (!row.rows[0]) throw new HttpError(404, "Gerät nicht gefunden");
      res.json(row.rows[0]);
    }),
  );
  router.delete(
    "/devices/:id",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      const owner = await ownerId(req);
      await pool.query(`DELETE FROM devices WHERE id = $1 AND user_id = $2`, [id, owner]);
      await pool.query(`UPDATE user_api_keys SET resources = array_remove(resources, $1) WHERE user_id = $2`, [id, owner]);
      res.status(204).end();
    }),
  );

  router.post(
    "/devices/:id/chat",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      await toVoice(res, `/internal/devices/${id}/chat`, {
        method: "POST",
        body: { text: parse(z.string().trim().min(1), req.body?.text) },
      });
    }),
  );
  router.post(
    "/transcribe",
    express.raw({ type: ["audio/wav", "audio/wave", "application/octet-stream"], limit: "8mb" }),
    handle(async (req, res) => {
      if (!Buffer.isBuffer(req.body) || req.body.length === 0) throw new HttpError(400, "Audio fehlt");
      await toVoiceBuffer(res, "/internal/transcribe", req.body, "audio/wav");
    }),
  );
  router.post(
    "/devices/:id/speak",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      await toVoice(res, `/internal/devices/${id}/speak`, {
        method: "POST",
        body: { text: parse(z.string().trim().min(1), req.body?.text) },
      });
    }),
  );
  router.get(
    "/devices/:id/runs",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      const rows = await pool.query(
        `SELECT id, channel, status, user_text, final_text, iterations, input_tokens, output_tokens, error, started_at, finished_at
         FROM agent_runs WHERE device_id = $1 ORDER BY started_at DESC LIMIT 20`,
        [id],
      );
      res.json({ runs: rows.rows });
    }),
  );
  router.get(
    "/runs/:id",
    handle(async (req, res) => {
      const run = await pool.query(
        `SELECT r.id FROM agent_runs r JOIN devices d ON d.id = r.device_id
         WHERE r.id = $1 AND d.user_id = $2`,
        [parse(uuid, req.params.id), await ownerId(req)],
      );
      if (!run.rows[0]) throw new HttpError(404, "Lauf nicht gefunden");
      const steps = await pool.query(
        `SELECT seq, kind, name, payload, duration_ms, error FROM agent_run_steps WHERE run_id = $1 ORDER BY seq`,
        [req.params.id],
      );
      res.json({ steps: steps.rows });
    }),
  );

  // ---------------------------------------------------------------- memory of a device

  router.get(
    "/devices/:id/memory",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      const device = await pool.query(`SELECT context FROM devices WHERE id = $1`, [id]);
      const messages = await pool.query(
        `SELECT id::text, role, content, metadata, created_at FROM device_messages WHERE device_id = $1 ORDER BY id DESC LIMIT 50`,
        [id],
      );
      res.json({ context: device.rows[0]?.context ?? "", messages: messages.rows.reverse() });
    }),
  );
  router.delete(
    "/devices/:id/memory",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      const what = parse(z.enum(["context", "messages", "all"]).default("all"), req.query.what);
      if (what !== "messages") await pool.query(`UPDATE devices SET context = '' WHERE id = $1`, [id]);
      if (what !== "context") await pool.query(`DELETE FROM device_messages WHERE device_id = $1`, [id]);
      res.status(204).end();
    }),
  );
  router.put(
    "/devices/:id/context",
    handle(async (req, res) => {
      const id = await ownedDevice(req, parse(uuid, req.params.id));
      const context = parse(z.string().trim().max(CONTEXT_CHARS), req.body?.context);
      await pool.query(`UPDATE devices SET context = $2 WHERE id = $1`, [id, context]);
      res.json({ context });
    }),
  );

  router.delete(
    "/devices/:id/messages/:messageId",
    handle(async (req, res) => {
      await pool.query(`DELETE FROM device_messages WHERE device_id = $1 AND id = $2`, [
        await ownedDevice(req, parse(uuid, req.params.id)),
        parse(z.coerce.number().int(), req.params.messageId),
      ]);
      res.status(204).end();
    }),
  );

  registerMcp(router);

  router.use((error: unknown, _req: Request, res: Response, _next: NextFunction) => {
    if (error instanceof HttpError) {
      res.status(error.status).json({ error: error.message });
      return;
    }
    console.error(error);
    res.status(500).json({ error: "Interner Fehler" });
  });

  return router;
}
