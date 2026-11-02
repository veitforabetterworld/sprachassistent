import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { ChevronRight } from "lucide-react";
import {
  VeitButton,
  VeitEmptyState,
  VeitField,
  VeitInput,
  VeitList,
  VeitListItem,
  VeitSecretField,
  VeitSelect,
  VeitSettingsSection,
  VeitSettingsToggleRow,
  VeitStatusBadge,
} from "@veit/react-controls";
import { VeitOverlayDialog, VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type McpInput, type McpServer, type McpTool, type NativeTool, type ToolEffect } from "../../api";
import type { MessageKey } from "../../i18n/de";
import { useI18n } from "../../i18n/I18nProvider";

const PRESET_URL: Record<string, string> = {
  homeassistant: "",
  calendar: "https://calendarmcp.googleapis.com/mcp/v1",
  gmail: "https://gmailmcp.googleapis.com/mcp/v1",
  custom: "",
};

const PRESET_AUTH: Record<string, string> = {
  homeassistant: "bearer",
  calendar: "oauth",
  gmail: "oauth",
  custom: "none",
};

const PRESET_LABEL: Record<string, MessageKey> = {
  homeassistant: "mcpPresetHa",
  calendar: "mcpPresetCalendar",
  gmail: "mcpPresetGmail",
  custom: "mcpPresetCustom",
};

const EFFECTS: ToolEffect[] = ["auto", "confirm", "deny"];
const EFFECT_LABEL: Record<ToolEffect, MessageKey> = {
  auto: "effectAuto",
  confirm: "effectConfirm",
  deny: "effectDeny",
};

type Draft = McpInput;

function emptyDraft(): Draft {
  return { name: "", preset: "homeassistant", transport: "http", url: "", command: "", enabled: true, auth_type: "bearer", secret: "" };
}

function McpEditDialog({
  open,
  server,
  onClose,
  onSaved,
}: {
  open: boolean;
  server: McpServer | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t, tf } = useI18n();
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo<Draft>(
    () =>
      server
        ? { name: server.name, preset: server.preset, transport: server.transport, url: server.url, command: server.command, enabled: server.enabled, auth_type: server.auth_type, secret: "" }
        : emptyDraft(),
    [server],
  );
  const [draft, setDraft] = useState<Draft>(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((d) => ({ ...d, [key]: value }));
  useEffect(() => {
    if (open) {
      setDraft(initial);
      setError(null);
    }
  }, [open, initial]);
  const { dirty, formBaseline } = useDialogFormBaseline({ draft, resolveBaseline: () => initial, resetDeps: [open, initial], track: open });

  const save = async () => {
    setSaving(true);
    try {
      if (server) await adminApi.updateMcp(server.id, draft);
      else await adminApi.createMcp(draft);
      onSaved();
    } catch (e) {
      setError(errorText(e));
      throw e;
    } finally {
      setSaving(false);
    }
  };

  return (
    <VeitOverlayEditDialog
      binding={binding}
      title={server ? server.name : t("mcpAdd")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveRequiresDirty: server !== null,
        cancelLabel: t("cancel"),
        saveLabel: server ? t("save") : t("add"),
        onSave: save,
        deleteAction: server
          ? {
              ariaLabel: t("mcpDelete"),
              confirm: {
                title: t("mcpDeleteTitle"),
                message: tf("mcpDeleteText", { name: server.name }),
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
                destructive: true,
              },
              onDelete: async () => {
                await adminApi.deleteMcp(server.id);
                onSaved();
              },
            }
          : undefined,
      }}
    >
      <div className="flex flex-col gap-4">
        <VeitField label={t("mcpPreset")} htmlFor="mcp-preset">
          <VeitSelect
            id="mcp-preset"
            value={draft.preset}
            disabled={saving}
            onValueChange={(value) =>
              setDraft((d) => ({
                ...d,
                preset: value,
                url: d.url || PRESET_URL[value] || "",
                auth_type: PRESET_AUTH[value] || "none",
              }))
            }
            options={Object.keys(PRESET_LABEL).map((value) => ({ value, label: t(PRESET_LABEL[value]) }))}
          />
        </VeitField>
        <VeitField label={t("name")} htmlFor="mcp-name">
          <VeitInput id="mcp-name" value={draft.name} disabled={saving} onChange={(e) => set("name", e.target.value)} />
        </VeitField>
        <VeitField label={t("mcpUrl")} htmlFor="mcp-url" hint={t("mcpUrlHint")}>
          <VeitInput id="mcp-url" value={draft.url} disabled={saving} onChange={(e) => set("url", e.target.value)} />
        </VeitField>
        {draft.auth_type === "bearer" && (
          <VeitField label={t("mcpToken")} htmlFor="mcp-token" hint={server?.secret_set ? t("mcpTokenStored") : undefined}>
            <VeitSecretField
              key={server?.id ?? "new"}
              id="mcp-token"
              value={draft.secret}
              onChange={(value) => set("secret", value)}
              stored={server?.secret_set ?? false}
              showLabel={t("showPassword")}
              hideLabel={t("hidePassword")}
              copyLabel={t("copy")}
              copiedLabel={t("copied")}
              disabled={saving}
            />
          </VeitField>
        )}
        {draft.transport === "stdio" && <p className="m-0 text-sm text-muted-foreground">{t("mcpStdio")}</p>}
        <VeitSettingsToggleRow variant="embed" label={t("mcpEnabled")} checked={draft.enabled} disabled={saving} onCheckedChange={(v) => set("enabled", v)} />
        {error && (
          <VeitStatusBadge variant="destructive" shape="rounded">
            {error}
          </VeitStatusBadge>
        )}
      </div>
    </VeitOverlayEditDialog>
  );
}

function effectOf(value: string | null): ToolEffect {
  return value === "confirm" || value === "deny" ? value : "auto";
}

type Permission = ToolEffect | "off";

function permissionOf(enabled: boolean, effect: string | null): Permission {
  return enabled ? effectOf(effect) : "off";
}

const GROUP_ORDER = ["home", "control", "calendar", "todo", "media", "time", "announce", "advanced", "other"] as const;
type ToolGroup = (typeof GROUP_ORDER)[number];

const GROUP_LABEL: Record<ToolGroup, MessageKey> = {
  home: "mcpGroupHome",
  control: "mcpGroupControl",
  calendar: "mcpGroupCalendar",
  todo: "mcpGroupTodo",
  media: "mcpGroupMedia",
  time: "mcpGroupTime",
  announce: "mcpGroupAnnounce",
  advanced: "mcpGroupAdvanced",
  other: "mcpGroupOther",
};

const TOOL_LABEL: Record<string, MessageKey> = {
  turnon: "toolTurnOn",
  turnoff: "toolTurnOff",
  setposition: "toolSetPosition",
  stopmoving: "toolStopMoving",
  cancelalltimers: "toolCancelTimers",
  getevents: "toolGetEvents",
  getlivecontext: "toolLiveContext",
  getdatetime: "toolDateTime",
  broadcast: "toolBroadcast",
  medianext: "toolMediaNext",
  mediaprevious: "toolMediaPrevious",
  mediapause: "toolMediaPause",
  mediaunpause: "toolMediaUnpause",
  mediaplayermute: "toolMediaMute",
  mediaplayerunmute: "toolMediaUnmute",
  setvolume: "toolSetVolume",
  setvolumerelative: "toolSetVolumeRelative",
  getitems: "toolGetItems",
  listadditem: "toolListAdd",
  listcompleteitem: "toolListComplete",
  listremoveitem: "toolListRemove",
  search: "toolSearch",
  getoverview: "toolOverview",
  configgetautomation: "toolAutomationRead",
  configsetautomation: "toolAutomationWrite",
  calltool: "toolCall",
  searchtools: "toolSearchTools",
  getskillguide: "toolGuide",
  reportissue: "toolReport",
};

function toolToken(name: string): string {
  const last = name.split("__").pop() ?? name;
  return last.replace(/^ha_/i, "").replace(/^hass/i, "").replace(/_/g, "").toLowerCase();
}

function toolGroup(name: string): ToolGroup {
  const value = name.toLowerCase();
  if (value.includes("ha-mcp-server-tool-search")) return "advanced";
  if (value.includes("calendar")) return "calendar";
  if (value.includes("todo")) return "todo";
  if (value.includes("media")) return "media";
  if (value.includes("datetime") || value.includes("__llm__")) return "time";
  if (value.includes("livecontext")) return "home";
  if (value.includes("broadcast") || value.includes("satellite")) return "announce";
  if (value.includes("intent")) return "control";
  return "other";
}

function readableTool(name: string, t: (key: MessageKey) => string): string {
  const key = TOOL_LABEL[toolToken(name)];
  if (key) return t(key);
  const last = (name.split("__").pop() ?? name).replace(/^ha_/i, "").replace(/^hass/i, "");
  const spaced = last.replace(/_/g, " ").replace(/([a-z])([A-Z])/g, "$1 $2").trim();
  return spaced ? spaced.charAt(0).toUpperCase() + spaced.slice(1) : name;
}

function PermissionSelect({ value, onChange }: { value: Permission; onChange: (value: Permission) => void }) {
  const { t } = useI18n();
  return (
    <div className="w-40">
      <VeitSelect
        value={value}
        aria-label={t("mcpEffect")}
        onValueChange={(next) => onChange(next as Permission)}
        options={[...EFFECTS.map((effect) => ({ value: effect, label: t(EFFECT_LABEL[effect]) })), { value: "off", label: t("mcpToolOff") }]}
      />
    </div>
  );
}

function ToolsDialog({
  server,
  tools,
  onClose,
  onEdit,
  onPermission,
}: {
  server: McpServer | null;
  tools: McpTool[];
  onClose: () => void;
  onEdit: () => void;
  onPermission: (tool: McpTool, permission: Permission) => void;
}) {
  const { t, tf } = useI18n();
  const binding = useManagedOverlayDialogBinding(server !== null, onClose);
  const [filter, setFilter] = useState("");
  useEffect(() => {
    if (server) setFilter("");
  }, [server]);
  const present = tools.filter((tool) => tool.present);
  const query = filter.trim().toLowerCase();
  const visible = present.filter((tool) => {
    if (!query) return true;
    return readableTool(tool.name, t).toLowerCase().includes(query) || t(GROUP_LABEL[toolGroup(tool.name)]).toLowerCase().includes(query);
  });
  const count = present.length;
  return (
    <VeitOverlayDialog binding={binding} title={server?.name ?? ""} closeAriaLabel={t("close")} variant="responsive" size="lg">
      {server && (
        <div className="flex flex-col gap-4">
          <p className="m-0 text-sm text-muted-foreground">{tf(count === 1 ? "mcpToolCountOne" : "mcpToolCount", { count })}</p>
          {server.last_error && (
            <VeitStatusBadge variant="destructive" shape="rounded">
              {server.last_error}
            </VeitStatusBadge>
          )}
          <div className="flex flex-wrap gap-2">
            {server.auth_type === "oauth" && (
              <VeitButton variant="secondary" onClick={() => window.location.assign(`/v1/admin/mcp/${server.id}/oauth/start`)}>
                {server.oauth_set ? t("mcpConnected") : t("mcpConnect")}
              </VeitButton>
            )}
            <VeitButton variant="secondary" onClick={onEdit}>
              {t("mcpEditServer")}
            </VeitButton>
          </div>
          {server.transport === "stdio" && <p className="m-0 text-sm text-muted-foreground">{t("mcpStdio")}</p>}
          <VeitInput value={filter} placeholder={t("mcpFilter")} aria-label={t("mcpFilter")} onChange={(event) => setFilter(event.target.value)} />
          {present.length === 0 && <VeitEmptyState embedded title={t("mcpNoTools")} />}
          {present.length > 0 && visible.length === 0 && <p className="m-0 text-sm text-muted-foreground">{t("mcpNoMatch")}</p>}
          {GROUP_ORDER.map((group) => {
            const rows = visible.filter((tool) => toolGroup(tool.name) === group);
            if (rows.length === 0) return null;
            return (
              <div key={group} className="flex flex-col gap-2">
                <p className="m-0 text-sm font-medium text-foreground">{t(GROUP_LABEL[group])}</p>
                <VeitList>
                  {rows.map((tool) => (
                    <VeitListItem
                      key={`${tool.server_id}-${tool.name}`}
                      title={readableTool(tool.name, t)}
                      trailing={
                        <PermissionSelect value={permissionOf(tool.enabled, tool.effect)} onChange={(permission) => onPermission(tool, permission)} />
                      }
                    />
                  ))}
                </VeitList>
              </div>
            );
          })}
        </div>
      )}
    </VeitOverlayDialog>
  );
}

export function McpPage() {
  const { t, tf } = useI18n();
  const [params] = useSearchParams();
  const [servers, setServers] = useState<McpServer[]>([]);
  const [tools, setTools] = useState<McpTool[]>([]);
  const [native, setNative] = useState<NativeTool[]>([]);
  const [editing, setEditing] = useState<McpServer | "new" | null>(null);
  const [browsingId, setBrowsingId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(async () => {
    const [mcp, builtin] = await Promise.all([adminApi.mcp(), adminApi.nativeTools()]);
    setServers(mcp.servers);
    setTools(mcp.tools);
    setNative(builtin.tools);
    return mcp.servers;
  }, []);

  const refreshServers = useCallback(
    async (list: McpServer[]) => {
      const live = list.filter((server) => server.enabled && server.transport !== "stdio");
      if (live.length === 0) return;
      setRefreshing(true);
      let failed = "";
      await Promise.all(
        live.map(async (server) => {
          try {
            const result = await adminApi.discoverMcp(server.id);
            if (!result.ok && result.error) failed = result.error;
          } catch (error) {
            failed = errorText(error);
          }
        }),
      );
      if (failed) setNotice(failed);
      await load();
      setRefreshing(false);
    },
    [load],
  );

  useEffect(() => {
    let cancel = false;
    void (async () => {
      try {
        const list = await load();
        if (!cancel) await refreshServers(list);
      } catch (error) {
        if (!cancel) setNotice(errorText(error));
      }
    })();
    return () => {
      cancel = true;
    };
  }, [load, refreshServers]);

  useEffect(() => {
    if (params.get("oauth") === "ok") setNotice(t("mcpOauthOk"));
    if (params.get("oauth") === "error") setNotice(t("mcpOauthError"));
  }, [params, t]);

  const saveTool = async (tool: McpTool, permission: Permission) => {
    const enabled = permission !== "off";
    const effect = permission === "off" ? effectOf(tool.effect) : permission;
    setTools((current) => current.map((item) => (item.server_id === tool.server_id && item.name === tool.name ? { ...item, enabled, effect } : item)));
    try {
      await adminApi.updateMcpTool(tool.server_id, tool.name, { enabled, effect });
    } catch (error) {
      setNotice(errorText(error));
      await load();
    }
  };

  const saveNative = async (tool: NativeTool, permission: Permission) => {
    const enabled = permission !== "off";
    const effect = permission === "off" ? tool.effect : permission;
    setNative((current) => current.map((item) => (item.name === tool.name ? { ...item, enabled, effect } : item)));
    try {
      await adminApi.updateNativeTool(tool.name, { enabled, effect });
    } catch (error) {
      setNotice(errorText(error));
      await load();
    }
  };

  const browsing = servers.find((server) => server.id === browsingId) ?? null;
  const nativeLabel = () => t("nativeResearch");
  const chevron = <ChevronRight className="size-4 text-muted-foreground" aria-hidden />;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      {notice && (
        <VeitStatusBadge variant="neutral" shape="rounded">
          {notice}
        </VeitStatusBadge>
      )}
      <VeitSettingsSection
        title={t("mcpTitle")}
        description={t("mcpHint")}
        actions={
          <VeitButton variant="secondary" onClick={() => setEditing("new")}>
            {t("mcpAdd")}
          </VeitButton>
        }
      >
        {servers.length === 0 ? (
          <VeitEmptyState embedded title={t("mcpEmpty")} />
        ) : (
          <VeitList>
            {servers.map((server) => {
              const count = tools.filter((tool) => tool.server_id === server.id && tool.present).length;
              const countLabel = tf(count === 1 ? "mcpToolCountOne" : "mcpToolCount", { count });
              return (
                <VeitListItem
                  key={server.id}
                  title={server.name}
                  meta={refreshing && count === 0 ? t("mcpLoading") : countLabel}
                  badge={
                    <VeitStatusBadge variant={server.last_error ? "destructive" : server.enabled ? "success" : "inactive"}>
                      {server.last_error ? t("error") : tf("mcpStatus", { status: server.status })}
                    </VeitStatusBadge>
                  }
                  trailing={chevron}
                  onClick={() => setBrowsingId(server.id)}
                />
              );
            })}
          </VeitList>
        )}
      </VeitSettingsSection>
      <VeitSettingsSection title={t("nativeTitle")} description={t("nativeHint")}>
        <VeitList>
          {native.map((tool) => (
            <VeitListItem
              key={tool.name}
              title={nativeLabel()}
              trailing={<PermissionSelect value={permissionOf(tool.enabled, tool.effect)} onChange={(permission) => void saveNative(tool, permission)} />}
            />
          ))}
        </VeitList>
      </VeitSettingsSection>
      <ToolsDialog
        server={browsing}
        tools={tools.filter((tool) => tool.server_id === browsingId)}
        onClose={() => setBrowsingId(null)}
        onEdit={() => {
          if (!browsing) return;
          setEditing(browsing);
          setBrowsingId(null);
        }}
        onPermission={(tool, permission) => void saveTool(tool, permission)}
      />
      <McpEditDialog
        open={editing !== null}
        server={editing === "new" ? null : editing}
        onClose={() => setEditing(null)}
        onSaved={() => {
          const id = editing !== "new" && editing ? editing.id : null;
          setEditing(null);
          void (async () => {
            const list = await load();
            await refreshServers(id ? list.filter((server) => server.id === id) : list);
          })();
        }}
      />
    </div>
  );
}
