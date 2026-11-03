import { Readable } from "node:stream";
import type { NextFunction, Request, Response } from "express";
import { z } from "zod";
import { env } from "./env.js";

/** Shared HTTP helpers for the admin and MCP routes. */

/**
 * Answered as `{ code, detail?, fields? }`. The code is a key the web words (apps/web/src/errors.ts);
 * detail is technical, fields name the inputs that failed validation.
 */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    readonly detail = "",
    readonly fields: string[] = [],
  ) {
    super(detail || code);
  }
}

/** The error handler at the end of a router. Anything without a code is only "internal" outside the log. */
export function sendError(error: unknown, _req: Request, res: Response, _next: NextFunction): void {
  if (error instanceof HttpError) {
    res.status(error.status).json({
      code: error.code,
      ...(error.detail ? { detail: error.detail } : {}),
      ...(error.fields.length ? { fields: error.fields } : {}),
    });
    return;
  }
  console.error(error);
  res.status(500).json({ code: "internal" });
}

type Handler = (req: Request, res: Response) => Promise<unknown>;

export const handle = (fn: Handler) => (req: Request, res: Response, next: NextFunction) => {
  fn(req, res).catch(next);
};

export function parse<T>(schema: z.ZodType<T>, body: unknown): T {
  const result = schema.safeParse(body);
  if (!result.success) {
    const issues = result.error.issues;
    throw new HttpError(
      400,
      "validation",
      issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; "),
      [...new Set(issues.map((i) => String(i.path[0] ?? "")).filter(Boolean))],
    );
  }
  return result.data;
}

/** Forwards to the voice server's internal routes and passes its answer through (JSON or audio). */
export async function toVoice(res: Response, path: string, init?: { method?: string; body?: unknown }) {
  let upstream: globalThis.Response;
  try {
    upstream = await fetch(`${env.voiceUrl}${path}`, {
      method: init?.method ?? "GET",
      headers: init?.body === undefined ? undefined : { "content-type": "application/json" },
      body: init?.body === undefined ? undefined : JSON.stringify(init.body),
    });
  } catch {
    throw new HttpError(502, "voice.unreachable");
  }
  res.status(upstream.status);
  for (const header of ["content-type", "x-sample-rate", "x-transcript"]) {
    const value = upstream.headers.get(header);
    if (value) res.setHeader(header, value);
  }
  if (upstream.body) Readable.fromWeb(upstream.body as import("node:stream/web").ReadableStream).pipe(res);
  else res.end();
}

/** Forwards a raw body, such as a WAV recording, and returns the JSON answer. */
export async function toVoiceBuffer(res: Response, path: string, body: Buffer, contentType: string): Promise<void> {
  let upstream: globalThis.Response;
  try {
    upstream = await fetch(`${env.voiceUrl}${path}`, {
      method: "POST",
      headers: { "content-type": contentType },
      body: new Uint8Array(body),
    });
  } catch {
    throw new HttpError(502, "voice.unreachable");
  }
  const text = await upstream.text();
  res.status(upstream.status);
  res.type(upstream.headers.get("content-type") ?? "application/json");
  res.send(text);
}
