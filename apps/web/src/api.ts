import { createAuthProvider, createRestAuthClient, useAuth, veitFetchJson, VeitApiError, type VeitWireUser } from "@veit/react-auth";
import { createRestAccountClient } from "@veit/react-account";
import { createRestApiKeysClient } from "@veit/react-api-keys";

/** Signed-in user as apps/api sends it (@veit/auth-server) plus the admin flag. */
export type AccountUser = VeitWireUser & { is_admin: boolean };

export const authClient = createRestAuthClient<AccountUser>("/v1", (wire) => wire as AccountUser);
export const accountClient = createRestAccountClient("/v1", veitFetchJson, (iso) => new Date(iso).toLocaleString("de-DE"));
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
};
export type DeviceInput = Omit<Device, "id">;

export type Provider = {
  id: string;
  name: string;
  base_url: string;
  model: string;
  enabled: boolean;
  priority: number;
  api_key_set: boolean;
  api_key_hint: string;
};
export type ProviderInput = Omit<Provider, "id" | "api_key_set" | "api_key_hint"> & { api_key: string };

export type Voice = { id: string; label: string };
export type Fact = { id: string; content: string; updated_at: string };
export type Message = { id: string; role: string; content: string; created_at: string };
export type Health = { db: boolean; stt: boolean; voices: string[]; providers: string[] };

export const adminApi = {
  status: () => admin<Health>("/status"),
  voices: async () => (await admin<{ voices: Voice[] }>("/voices")).voices,

  settings: async () => (await admin<{ values: Record<string, string | boolean> }>("/settings")).values,
  saveSettings: async (values: Record<string, string>) =>
    (await admin<{ values: Record<string, string | boolean> }>("/settings", "PUT", { values })).values,

  providers: async () => (await admin<{ providers: Provider[] }>("/providers")).providers,
  createProvider: (input: ProviderInput) => admin<Provider>("/providers", "POST", input),
  updateProvider: (id: string, input: ProviderInput) => admin<Provider>(`/providers/${id}`, "PUT", input),
  deleteProvider: (id: string) => admin<null>(`/providers/${id}`, "DELETE"),
  testProvider: (id: string) =>
    admin<{ ok: boolean; reply?: string; error?: string; ms: number }>(`/providers/${id}/test`, "POST", {}),

  devices: async () => (await admin<{ devices: Device[] }>("/devices")).devices,
  createDevice: (input: DeviceInput) => admin<Device>("/devices", "POST", input),
  updateDevice: (id: string, input: DeviceInput) => admin<Device>(`/devices/${id}`, "PUT", input),
  deleteDevice: (id: string) => admin<null>(`/devices/${id}`, "DELETE"),
  chat: (id: string, text: string) => admin<{ reply: string; ms: number }>(`/devices/${id}/chat`, "POST", { text }),

  memory: (id: string) => admin<{ facts: Fact[]; messages: Message[] }>(`/devices/${id}/memory`),
  clearMemory: (id: string, what: "facts" | "messages" | "all") => admin<null>(`/devices/${id}/memory?what=${what}`, "DELETE"),
  addFact: (id: string, content: string) => admin<Fact>(`/devices/${id}/facts`, "POST", { content }),
  updateFact: (id: string, factId: string, content: string) => admin<Fact>(`/devices/${id}/facts/${factId}`, "PUT", { content }),
  deleteFact: (id: string, factId: string) => admin<null>(`/devices/${id}/facts/${factId}`, "DELETE"),
  deleteMessage: (id: string, messageId: string) => admin<null>(`/devices/${id}/messages/${messageId}`, "DELETE"),
};

/** The device's voice as a playable blob URL (POST, so not a plain <audio src>). */
export async function speakUrl(deviceId: string, text: string): Promise<string> {
  const res = await fetch(`/v1/admin/devices/${deviceId}/speak`, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });
  if (!res.ok) throw new Error(`Sprachausgabe: HTTP ${res.status}`);
  // The voice server streams a WAV of unknown length; browsers play it once the sizes are filled in.
  const wav = new Uint8Array(await res.arrayBuffer());
  const view = new DataView(wav.buffer);
  view.setUint32(4, wav.length - 8, true);
  view.setUint32(40, wav.length - 44, true);
  return URL.createObjectURL(new Blob([wav], { type: "audio/wav" }));
}
