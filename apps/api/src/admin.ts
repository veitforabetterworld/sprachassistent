import { Readable } from "node:stream";
import { Router, type NextFunction, type Request, type Response } from "express";
import { z } from "zod";
import { adminDecision } from "./accounts.js";
import { pool } from "./db.js";
import { env } from "./env.js";
import { PASSWORD_SETTINGS, getSetting, listSettings, saveSettings } from "./settings.js";

/** Admin area: settings, AI providers, devices with voice, personality and memory. Admins only. */

type Handler = (req: Request, res: Response) => Promise<unknown>;

const handle = (fn: Handler) => (req: Request, res: Response, next: NextFunction) => {
  fn(req, res).catch(next);
};

class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

function parse<T>(schema: z.ZodType<T>, body: unknown): T {
  const result = schema.safeParse(body);
  if (!result.success) throw new HttpError(400, result.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; "));
  return result.data;
}

const uuid = z.string().uuid();

/** Forwards to the voice server's internal routes and passes its answer through (JSON or audio). */
async function toVoice(res: Response, path: string, init?: { method?: string; body?: unknown }) {
  let upstream: globalThis.Response;
  try {
    upstream = await fetch(`${env.voiceUrl}${path}`, {
      method: init?.method ?? "GET",
      headers: init?.body === undefined ? undefined : { "content-type": "application/json" },
      body: init?.body === undefined ? undefined : JSON.stringify(init.body),
    });
  } catch {
    throw new HttpError(502, "Sprachserver nicht erreichbar");
  }
  res.status(upstream.status);
  for (const header of ["content-type", "x-sample-rate", "x-transcript"]) {
    const value = upstream.headers.get(header);
    if (value) res.setHeader(header, value);
  }
  if (upstream.body) Readable.fromWeb(upstream.body as import("node:stream/web").ReadableStream).pipe(res);
  else res.end();
}

const providerSchema = z.object({
  name: z.string().trim().min(1).max(80),
  base_url: z.string().trim().url(),
  /** Empty keeps the stored key. */
  api_key: z.string().trim().default(""),
  model: z.string().trim().min(1).max(120),
  enabled: z.boolean().default(true),
  priority: z.coerce.number().int().default(0),
});

const deviceSchema = z.object({
  name: z.string().trim().min(1).max(80),
  voice: z.string().trim().min(1),
  speed: z.coerce.number().min(0.5).max(2).default(1),
  persona: z.string().max(8000).default(""),
  provider_id: uuid.nullable().default(null),
  history_messages: z.coerce.number().int().min(0).max(100).default(12),
  memory_extraction: z.boolean().default(true),
});

const PROVIDER_COLUMNS = `id, name, base_url, model, enabled, priority, api_key <> '' AS api_key_set,
  CASE WHEN length(api_key) > 8 THEN '…' || right(api_key, 4) ELSE '' END AS api_key_hint`;
const DEVICE_COLUMNS = `id, name, voice, speed, persona, provider_id, history_messages, memory_extraction, created_at`;

export function adminRouter(): Router {
  const router = Router();

  router.use((req, res, next) => {
    adminDecision(req)
      .then((decision) => (decision.ok ? next() : res.status(decision.status).json({ error: decision.error })))
      .catch(next);
  });

  router.get("/status", handle((_req, res) => toVoice(res, "/v1/health")));
  router.get("/voices", handle((_req, res) => toVoice(res, "/internal/voices")));

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
    handle(async (_req, res) => {
      const rows = await pool.query(`SELECT ${PROVIDER_COLUMNS} FROM llm_providers ORDER BY priority, name`);
      res.json({ providers: rows.rows });
    }),
  );
  router.post(
    "/providers",
    handle(async (req, res) => {
      const p = parse(providerSchema, req.body);
      const row = await pool.query(
        `INSERT INTO llm_providers (name, base_url, api_key, model, enabled, priority)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING ${PROVIDER_COLUMNS}`,
        [p.name, p.base_url, p.api_key, p.model, p.enabled, p.priority],
      );
      res.status(201).json(row.rows[0]);
    }),
  );
  router.put(
    "/providers/:id",
    handle(async (req, res) => {
      const id = parse(uuid, req.params.id);
      const p = parse(providerSchema, req.body);
      const row = await pool.query(
        `UPDATE llm_providers SET name = $2, base_url = $3, api_key = CASE WHEN $4 = '' THEN api_key ELSE $4 END,
           model = $5, enabled = $6, priority = $7
         WHERE id = $1 RETURNING ${PROVIDER_COLUMNS}`,
        [id, p.name, p.base_url, p.api_key, p.model, p.enabled, p.priority],
      );
      if (!row.rows[0]) throw new HttpError(404, "Anbieter nicht gefunden");
      res.json(row.rows[0]);
    }),
  );
  router.delete(
    "/providers/:id",
    handle(async (req, res) => {
      await pool.query(`DELETE FROM llm_providers WHERE id = $1`, [parse(uuid, req.params.id)]);
      res.status(204).end();
    }),
  );
  // The stored key, to show and copy it in the edit dialog.
  router.get(
    "/providers/:id/key",
    handle(async (req, res) => {
      const row = await pool.query<{ api_key: string }>(`SELECT api_key FROM llm_providers WHERE id = $1`, [parse(uuid, req.params.id)]);
      if (!row.rows[0]) throw new HttpError(404, "Anbieter nicht gefunden");
      res.set("Cache-Control", "no-store").json({ key: row.rows[0].api_key });
    }),
  );
  router.post(
    "/providers/:id/test",
    handle((req, res) => toVoice(res, `/internal/providers/${parse(uuid, req.params.id)}/test`, { method: "POST", body: {} })),
  );

  // ---------------------------------------------------------------- devices

  router.get(
    "/devices",
    handle(async (_req, res) => {
      const rows = await pool.query(`SELECT ${DEVICE_COLUMNS} FROM devices ORDER BY created_at`);
      res.json({ devices: rows.rows });
    }),
  );
  router.post(
    "/devices",
    handle(async (req, res) => {
      const d = parse(deviceSchema, req.body);
      const row = await pool.query(
        `INSERT INTO devices (name, voice, speed, persona, provider_id, history_messages, memory_extraction)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING ${DEVICE_COLUMNS}`,
        [d.name, d.voice, d.speed, d.persona, d.provider_id, d.history_messages, d.memory_extraction],
      );
      res.status(201).json(row.rows[0]);
    }),
  );
  router.put(
    "/devices/:id",
    handle(async (req, res) => {
      const id = parse(uuid, req.params.id);
      const d = parse(deviceSchema, req.body);
      const row = await pool.query(
        `UPDATE devices SET name = $2, voice = $3, speed = $4, persona = $5, provider_id = $6, history_messages = $7,
           memory_extraction = $8
         WHERE id = $1 RETURNING ${DEVICE_COLUMNS}`,
        [id, d.name, d.voice, d.speed, d.persona, d.provider_id, d.history_messages, d.memory_extraction],
      );
      if (!row.rows[0]) throw new HttpError(404, "Gerät nicht gefunden");
      res.json(row.rows[0]);
    }),
  );
  router.delete(
    "/devices/:id",
    handle(async (req, res) => {
      const id = parse(uuid, req.params.id);
      await pool.query(`DELETE FROM devices WHERE id = $1`, [id]);
      // Keys that were limited to this device lose it (framework store works per user, devices are global).
      await pool.query(`UPDATE user_api_keys SET resources = array_remove(resources, $1)`, [id]);
      res.status(204).end();
    }),
  );

  router.post(
    "/devices/:id/chat",
    handle((req, res) =>
      toVoice(res, `/internal/devices/${parse(uuid, req.params.id)}/chat`, {
        method: "POST",
        body: { text: parse(z.string().trim().min(1), req.body?.text) },
      }),
    ),
  );
  router.post(
    "/devices/:id/speak",
    handle((req, res) =>
      toVoice(res, `/internal/devices/${parse(uuid, req.params.id)}/speak`, {
        method: "POST",
        body: { text: parse(z.string().trim().min(1), req.body?.text) },
      }),
    ),
  );

  // ---------------------------------------------------------------- memory of a device

  router.get(
    "/devices/:id/memory",
    handle(async (req, res) => {
      const id = parse(uuid, req.params.id);
      const facts = await pool.query(
        `SELECT id::text, content, updated_at FROM device_facts WHERE device_id = $1 ORDER BY id`,
        [id],
      );
      const messages = await pool.query(
        `SELECT id::text, role, content, created_at FROM device_messages WHERE device_id = $1 ORDER BY id DESC LIMIT 50`,
        [id],
      );
      res.json({ facts: facts.rows, messages: messages.rows.reverse() });
    }),
  );
  router.delete(
    "/devices/:id/memory",
    handle(async (req, res) => {
      const id = parse(uuid, req.params.id);
      const what = parse(z.enum(["facts", "messages", "all"]).default("all"), req.query.what);
      if (what !== "messages") await pool.query(`DELETE FROM device_facts WHERE device_id = $1`, [id]);
      if (what !== "facts") await pool.query(`DELETE FROM device_messages WHERE device_id = $1`, [id]);
      res.status(204).end();
    }),
  );
  router.post(
    "/devices/:id/facts",
    handle(async (req, res) => {
      const row = await pool.query(
        `INSERT INTO device_facts (device_id, content) VALUES ($1, $2) RETURNING id::text, content, updated_at`,
        [parse(uuid, req.params.id), parse(z.string().trim().min(1).max(500), req.body?.content)],
      );
      res.status(201).json(row.rows[0]);
    }),
  );
  router.put(
    "/devices/:id/facts/:factId",
    handle(async (req, res) => {
      const row = await pool.query(
        `UPDATE device_facts SET content = $3, updated_at = NOW() WHERE device_id = $1 AND id = $2
         RETURNING id::text, content, updated_at`,
        [parse(uuid, req.params.id), parse(z.coerce.number().int(), req.params.factId), parse(z.string().trim().min(1).max(500), req.body?.content)],
      );
      if (!row.rows[0]) throw new HttpError(404, "Eintrag nicht gefunden");
      res.json(row.rows[0]);
    }),
  );
  router.delete(
    "/devices/:id/facts/:factId",
    handle(async (req, res) => {
      await pool.query(`DELETE FROM device_facts WHERE device_id = $1 AND id = $2`, [
        parse(uuid, req.params.id),
        parse(z.coerce.number().int(), req.params.factId),
      ]);
      res.status(204).end();
    }),
  );

  router.delete(
    "/devices/:id/messages/:messageId",
    handle(async (req, res) => {
      await pool.query(`DELETE FROM device_messages WHERE device_id = $1 AND id = $2`, [
        parse(uuid, req.params.id),
        parse(z.coerce.number().int(), req.params.messageId),
      ]);
      res.status(204).end();
    }),
  );

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
