import { VeitApiError, veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import type { MessageKey } from "./i18n/de";
import { getActiveLocale } from "./i18n/locale";
import { tr, trFormat } from "./i18n/translate";

/** An answer of apps/api or the voice server: `{ code, detail?, fields? }`, see apps/api/src/http.ts. */
export class AdminApiError extends VeitApiError {
  constructor(
    status: number,
    code: string,
    readonly detail: string,
    readonly fields: string[],
  ) {
    super(status, code);
  }
}

/** The result of a test against a model or a tool server; `detail` is what the other side said. */
export type Failure = { code: string; detail?: string };

/** Our sentence per server code (apps/api, src/failure.rs). */
const CODE_KEY: Record<string, MessageKey> = {
  internal: "errInternal",
  validation: "errValidation",
  "admin.required": "errAdminRequired",
  "account.delete_self": "errDeleteSelf",
  "device.not_found": "errDeviceNotFound",
  "provider.not_found": "errProviderNotFound",
  "run.not_found": "errRunNotFound",
  "tool.not_found": "errToolNotFound",
  "secrets.key_missing": "errSecretsKeyMissing",
  "voice.unreachable": "errVoiceUnreachable",
  "speech.no_audio": "errSpeechNoAudio",
  "speech.bad_audio": "errSpeechBadAudio",
  "speech.upload_broken": "errSpeechUploadBroken",
  "speech.recording_too_long": "errSpeechTooLong",
  "speech.stt_unavailable": "errSttUnavailable",
  "speech.voice_unavailable": "errVoiceUnavailable",
  "llm.not_configured": "errLlmNotConfigured",
  "llm.unauthorized": "errLlmUnauthorized",
  "llm.model_not_found": "errLlmModelNotFound",
  "llm.too_large": "errLlmTooLarge",
  "llm.at_limit": "errLlmAtLimit",
  "llm.bad_request": "errLlmBadRequest",
  "llm.unavailable": "errLlmUnavailable",
  "llm.timeout": "errLlmTimeout",
  "llm.unreachable": "errLlmUnreachable",
  "llm.empty_reply": "errLlmEmptyReply",
  "llm.failed": "errLlmFailed",
  "mcp.not_found": "errMcpNotFound",
  "mcp.disabled": "errMcpDisabled",
  "mcp.stdio_disabled": "errMcpStdioDisabled",
  "mcp.no_url": "errMcpNoUrl",
  "mcp.timeout": "errMcpTimeout",
  "mcp.unauthorized": "errMcpUnauthorized",
  "mcp.unreachable": "errMcpUnreachable",
  "mcp.failed": "errMcpFailed",
  "mcp.not_oauth": "errMcpNotOauth",
  "mcp.oauth_not_connected": "errMcpOauthNotConnected",
  "mcp.oauth_not_configured": "errMcpOauthNotConfigured",
  "mcp.oauth_refresh_failed": "errMcpOauthRefreshFailed",
  "api_keys.one_device": "keysOneDevice",
};

/** Field names of the forms as the server reports them. Unknown ones are shown as they are. */
const FIELD_KEY: Record<string, MessageKey> = {
  name: "name",
  base_url: "baseUrl",
  model: "model",
  api_key: "apiKey",
  voice: "voice",
  speed: "speed",
  persona: "persona",
  max_iterations: "maxIterations",
  busy_phrase: "busyPhrase",
  still_busy_phrase: "stillBusyPhrase",
  url: "mcpUrl",
  context: "contextTitle",
  text: "testText",
};

/** The sentence for a server code. Codes of the auth server come with their own texts. */
export function codeText(code: string): string {
  const key = CODE_KEY[code];
  if (key) return tr(key);
  if (code.startsWith("auth.") || code.startsWith("api_keys.")) {
    return veitAuthErrorText({ code }, veitAuthT(getActiveLocale(), { internal: tr("errInternal") }));
  }
  if (/^http_5\d\d$/.test(code)) return tr("errServerUnavailable");
  return tr("errInternal");
}

/** A readable sentence for anything a request can throw; never the raw code or a stack. */
export function errorText(error: unknown): string {
  if (error instanceof AdminApiError && error.code === "validation" && error.fields.length > 0) {
    const fields = error.fields.map((field) => (FIELD_KEY[field] ? tr(FIELD_KEY[field]) : field));
    return trFormat("errValidationFields", { fields: fields.join(", ") });
  }
  if (error instanceof VeitApiError) return codeText(error.code);
  // fetch throws a TypeError when the server cannot be reached at all.
  if (error instanceof TypeError) return tr("errOffline");
  return tr("errInternal");
}

/** Why the browser gave no microphone. */
export function micErrorText(error: unknown): string {
  const name = error instanceof DOMException ? error.name : "";
  if (name === "NotAllowedError" || name === "SecurityError") return tr("errMicDenied");
  if (name === "NotFoundError" || name === "OverconstrainedError") return tr("errMicMissing");
  return tr("errMicFailed");
}

/** The technical detail behind an error, for the "Details" line; empty when there is none. */
export function errorDetail(error: unknown): string {
  return error instanceof AdminApiError ? error.detail : "";
}

/** JSON request to the admin API; failures become AdminApiError with code and detail. */
export async function fetchAdminJson<T>(url: string, init?: { method?: string; body?: unknown }): Promise<T> {
  const res = await fetch(url, {
    method: init?.method ?? "GET",
    credentials: "include",
    headers: init?.body === undefined ? undefined : { "Content-Type": "application/json" },
    body: init?.body === undefined ? undefined : JSON.stringify(init.body),
  });
  return readJson<T>(res);
}

function parseBody(text: string): { code?: unknown; detail?: unknown; fields?: unknown } | null {
  if (!text) return null;
  try {
    return JSON.parse(text) as { code?: unknown; detail?: unknown; fields?: unknown };
  } catch {
    return null;
  }
}

/** The JSON body of an answer, or AdminApiError for a status outside 2xx. */
export async function readJson<T>(res: Response): Promise<T> {
  const text = await res.text();
  const body = parseBody(text);
  if (!res.ok) {
    throw new AdminApiError(
      res.status,
      typeof body?.code === "string" ? body.code : `http_${res.status}`,
      typeof body?.detail === "string" ? body.detail : "",
      Array.isArray(body?.fields) ? body.fields.map(String) : [],
    );
  }
  return body as T;
}
