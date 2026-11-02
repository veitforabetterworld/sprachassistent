import { createAuthProvider, createRestAuthClient, useAuth, veitFetchJson, VeitApiError, type VeitWireUser } from "@veit/react-auth";
import { createRestAccountClient } from "@veit/react-account";
import { createRestApiKeysClient } from "@veit/react-api-keys";
import { formatLocalizedDateTime } from "@veit/intl";
import { getActiveLocale } from "./i18n/locale";
import { trFormat } from "./i18n/translate";

/** Signed-in user as apps/api sends it (@veit/auth-server) plus the admin flag. */
export type AccountUser = VeitWireUser & { is_admin: boolean; verification_required: boolean };

export const authClient = createRestAuthClient<AccountUser>("/v1", (wire) => wire as AccountUser);
export const accountClient = createRestAccountClient("/v1", veitFetchJson, (iso) => formatLocalizedDateTime(getActiveLocale(), iso));
export const apiKeysClient = createRestApiKeysClient("/v1", veitFetchJson);
export const AuthProvider = createAuthProvider<AccountUser>(authClient);

export function useAccount() {
  return useAuth<AccountUser>();
}

export function errorText(error: unknown): string {
  return error instanceof VeitApiError ? error.code : String(error);
}

const admin = <T>(path: string, method = "GET", body?: unknown) =>
  veitFetchJson<T>(`/v1/admin${path}`, { method, body });

export type Device = {
  id: string;
  name: string;
  voice: string;
  speed: number;
  persona: string;
  provider_id: string | null;
  history_messages: number;
  memory_extraction: boolean;
  max_iterations: number;
  busy_phrase: string;
  still_busy_phrase: string;
  tools_enabled: boolean;
};
export type DeviceInput = Omit<Device, "id">;

export type Provider = {
  id: string;
  name: string;
  base_url: string;
  model: string;
  enabled: boolean;
  priority: number;
  supports_tools: boolean;
  api_key_set: boolean;
  api_key_hint: string;
};
export type ProviderInput = Omit<Provider, "id" | "api_key_set" | "api_key_hint"> & { api_key: string };

export type Voice = { id: string; label: string };
export type Fact = { id: string; content: string; origin: string; updated_at: string };
export type ToolCallMeta = { id?: string; name?: string; arguments?: string };
export type Message = {
  id: string;
  role: string;
  content: string;
  created_at: string;
  metadata?: { tool_calls?: ToolCallMeta[]; name?: string; tool_call_id?: string };
};
export type Health = { db: boolean; stt: boolean; voices: string[] };
export type ToolEffect = "auto" | "confirm" | "deny";
export type McpServer = {
  id: string;
  name: string;
  preset: string;
  transport: string;
  url: string;
  command: string;
  enabled: boolean;
  auth_type: string;
  secret_set: boolean;
  oauth_set: boolean;
  status: string;
  last_error: string;
  last_ok_at: string | null;
};
export type McpInput = {
  name: string;
  preset: string;
  transport: string;
  url: string;
  command: string;
  enabled: boolean;
  auth_type: string;
  secret: string;
};
export type McpTool = {
  server_id: string;
  name: string;
  description: string;
  enabled: boolean;
  effect: string | null;
  present: boolean;
};
export type NativeTool = { name: string; effect: ToolEffect; enabled: boolean };
export type AdminAccount = {
  id: string;
  email: string;
  first_name: string;
  last_name: string;
  email_verified_at: string | null;
  is_admin: boolean;
  created_at: string;
};

export const adminApi = {
  status: () => admin<Health>("/status"),
  voices: async () => (await admin<{ voices: Voice[] }>("/voices")).voices,

  users: () => admin<{ users: AdminAccount[]; verification_required: boolean }>("/users"),
  setVerificationRequired: (enabled: boolean) =>
    admin<{ verification_required: boolean }>("/users/verification", "PUT", { enabled }),
  approveUser: (id: string) => admin<AdminAccount>(`/users/${id}/approve`, "POST", {}),
  resetUserPassword: (id: string) => admin<{ ok: boolean }>(`/users/${id}/password-reset`, "POST", {}),
  deleteUser: (id: string) => admin<null>(`/users/${id}`, "DELETE"),

  settings: async () => (await admin<{ values: Record<string, string | boolean> }>("/settings")).values,
  settingSecret: async (key: string) => (await admin<{ value: string }>(`/settings/${key}/secret`)).value,
  saveSettings: async (values: Record<string, string>) =>
    (await admin<{ values: Record<string, string | boolean> }>("/settings", "PUT", { values })).values,

  providers: async () => (await admin<{ providers: Provider[] }>("/providers")).providers,
  createProvider: (input: ProviderInput) => admin<Provider>("/providers", "POST", input),
  updateProvider: (id: string, input: ProviderInput) => admin<Provider>(`/providers/${id}`, "PUT", input),
  deleteProvider: (id: string) => admin<null>(`/providers/${id}`, "DELETE"),
  providerKey: async (id: string) => (await admin<{ key: string }>(`/providers/${id}/key`)).key,
  testProvider: (id: string) =>
    admin<{ ok: boolean; reply?: string; error?: string; ms: number }>(`/providers/${id}/test`, "POST", {}),
  testProviderTools: (id: string) =>
    admin<{ ok: boolean; called?: boolean; reply?: string; error?: string; ms: number }>(`/providers/${id}/tool-test`, "POST", {}),
  providerChoices: async () => (await admin<{ providers: { id: string; name: string }[] }>("/provider-choices")).providers,

  devices: async () => (await admin<{ devices: Device[] }>("/devices")).devices,
  createDevice: (input: DeviceInput) => admin<Device>("/devices", "POST", input),
  updateDevice: (id: string, input: DeviceInput) => admin<Device>(`/devices/${id}`, "PUT", input),
  deleteDevice: (id: string) => admin<null>(`/devices/${id}`, "DELETE"),
  chat: (id: string, text: string) =>
    admin<{ reply: string; ms: number; run_id: string | null; status: string }>(`/devices/${id}/chat`, "POST", { text }),
  runs: (id: string) =>
    admin<{
      runs: {
        id: string;
        channel: string;
        status: string;
        user_text: string;
        final_text: string;
        iterations: number;
        input_tokens: number;
        output_tokens: number;
        error: string;
        started_at: string;
        finished_at: string | null;
      }[];
    }>(`/devices/${id}/runs`),
  runSteps: (id: string) =>
    admin<{ steps: { seq: number; kind: string; name: string; payload: unknown; duration_ms: number; error: string }[] }>(`/runs/${id}`),

  mcp: () =>
    admin<{
      servers: McpServer[];
      tools: McpTool[];
    }>("/mcp"),
  createMcp: (input: McpInput) => admin<McpServer>("/mcp", "POST", input),
  updateMcp: (id: string, input: McpInput) => admin<McpServer>(`/mcp/${id}`, "PUT", input),
  deleteMcp: (id: string) => admin<null>(`/mcp/${id}`, "DELETE"),
  discoverMcp: (id: string) => admin<{ ok: boolean; tools?: number; error?: string }>(`/mcp/${id}/discover`, "POST", {}),
  updateMcpTool: (serverId: string, name: string, body: { enabled: boolean; effect: ToolEffect }) =>
    admin<McpTool>(`/mcp/${encodeURIComponent(serverId)}/tools/${encodeURIComponent(name)}`, "PUT", body),
  nativeTools: () => admin<{ tools: NativeTool[] }>("/tools/native"),
  updateNativeTool: (name: string, body: { enabled: boolean; effect: ToolEffect }) =>
    admin<NativeTool>(`/tools/native/${name}`, "PUT", body),

  memory: (id: string) => admin<{ facts: Fact[]; messages: Message[] }>(`/devices/${id}/memory`),
  clearMemory: (id: string, what: "facts" | "messages" | "all") => admin<null>(`/devices/${id}/memory?what=${what}`, "DELETE"),
  addFact: (id: string, content: string) => admin<Fact>(`/devices/${id}/facts`, "POST", { content }),
  updateFact: (id: string, factId: string, content: string) => admin<Fact>(`/devices/${id}/facts/${factId}`, "PUT", { content }),
  deleteFact: (id: string, factId: string) => admin<null>(`/devices/${id}/facts/${factId}`, "DELETE"),
  deleteMessage: (id: string, messageId: string) => admin<null>(`/devices/${id}/messages/${messageId}`, "DELETE"),
};

/** Same recognizer as the speaker on the Pi. Empty text means silence. */
export async function transcribe(wav: Blob): Promise<string> {
  const res = await fetch("/v1/admin/transcribe", {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "audio/wav" }, // i18n-ignore: mime type
    body: wav,
  });
  const data = (await res.json().catch(() => ({}))) as { text?: string; error?: string };
  if (!res.ok) throw new Error(data.error || trFormat("speechError", { status: res.status }));
  return data.text ?? "";
}

/** The device's voice as a playable blob URL (POST, so not a plain <audio src>). */
export async function speakUrl(deviceId: string, text: string): Promise<string> {
  const res = await fetch(`/v1/admin/devices/${deviceId}/speak`, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });
  if (!res.ok) throw new Error(trFormat("speechError", { status: res.status }));
  // The voice server streams a WAV of unknown length; browsers play it once the sizes are filled in.
  const wav = new Uint8Array(await res.arrayBuffer());
  const view = new DataView(wav.buffer);
  view.setUint32(4, wav.length - 8, true);
  view.setUint32(40, wav.length - 44, true);
  return URL.createObjectURL(new Blob([wav], { type: "audio/wav" }));
}
