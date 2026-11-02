import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VEIT_SETTINGS_ACTIONS,
  VeitButton,
  VeitDeleteButton,
  VeitEmptyState,
  VeitField,
  VeitInput,
  VeitList,
  VeitListItem,
  VeitSettingsSection,
  VeitStatusBadge,
  VeitTabBar,
  VeitTextarea,
} from "@veit/react-controls";
import { VeitOverlayDialog, VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, speakUrl, type Device, type Fact, type Message, type Provider, type Voice } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";
import { DeviceEditDialog } from "./DeviceEditDialog";

type Notice = { ok: boolean; text: string } | null;

function NoticeBadge({ notice }: { notice: Notice }) {
  if (!notice) return null;
  return (
    <VeitStatusBadge variant={notice.ok ? "success" : "destructive"} shape="rounded">
      {notice.text}
    </VeitStatusBadge>
  );
}

const chevron = <ChevronRight className="size-4 text-muted-foreground" aria-hidden />;

/** Overview of a device; a click opens the edit dialog. */
function DeviceSummary({ device, voices, providers, onClick }: { device: Device; voices: Voice[]; providers: Provider[]; onClick: () => void }) {
  const { t, tf } = useI18n();
  const voice = voices.find((v) => v.id === device.voice)?.label ?? device.voice;
  const provider = providers.find((p) => p.id === device.provider_id)?.name ?? t("defaultProvider");
  return (
    <VeitList>
      <VeitListItem
        title={device.name}
        meta={
          <div className="flex flex-col gap-1">
            <span>{tf("deviceSummary", { voice, speed: device.speed, provider, memory: device.memory_extraction ? t("on") : t("off") })}</span>
            <span className="line-clamp-2">{device.persona || t("noPersona")}</span>
            <span>{tf("deviceId", { id: device.id })}</span>
          </div>
        }
        trailing={chevron}
        onClick={onClick}
      />
    </VeitList>
  );
}

/** Text chat and voice preview with the saved settings of the device. */
function DeviceTest({ device, onMemoryChanged }: { device: Device; onMemoryChanged: () => void }) {
  const { t, tf } = useI18n();
  const [text, setText] = useState(() => t("testDefault"));
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice>(null);
  const [audio, setAudio] = useState<string | null>(null);

  const run = async (action: "chat" | "speak") => {
    setBusy(true);
    setNotice(null);
    setAudio(null);
    try {
      let spoken = text;
      if (action === "chat") {
        const r = await adminApi.chat(device.id, text);
        setNotice({ ok: true, text: tf("replyWithTime", { reply: r.reply, ms: r.ms }) });
        onMemoryChanged();
        spoken = r.reply;
      }
      setAudio(await speakUrl(device.id, spoken));
    } catch (error) {
      // A failed voice keeps the text reply visible.
      setNotice((current) => (current?.ok ? { ok: true, text: `${current.text} · ${errorText(error)}` } : { ok: false, text: errorText(error) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <VeitSettingsSection title={t("testTitle")} description={t("testHint")}>
      <div className="flex flex-col gap-4">
        <VeitField label={t("testText")} htmlFor="device-test">
          <VeitTextarea id="device-test" rows={2} value={text} onChange={(e) => setText(e.target.value)} />
        </VeitField>
        <div className="flex flex-wrap gap-2">
          <VeitButton disabled={busy || !text.trim()} onClick={() => void run("chat")}>
            {t("ask")}
          </VeitButton>
          <VeitButton variant="secondary" disabled={busy || !text.trim()} onClick={() => void run("speak")}>
            {t("readAloud")}
          </VeitButton>
        </div>
        <NoticeBadge notice={notice} />
        {audio && <audio src={audio} controls autoPlay className="w-full" />}
      </div>
    </VeitSettingsSection>
  );
}

/** Adds a fact (fact null) or changes one. */
function FactEditDialog({
  open,
  device,
  fact,
  onClose,
  onSaved,
}: {
  open: boolean;
  device: Device;
  fact: Fact | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useI18n();
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(() => ({ content: fact?.content ?? "" }), [fact]);
  const [draft, setDraft] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
      if (fact) await adminApi.updateFact(device.id, fact.id, draft.content);
      else await adminApi.addFact(device.id, draft.content);
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
      title={fact ? t("factEdit") : t("factAdd")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveDisabled: !draft.content.trim(),
        cancelLabel: t("cancel"),
        saveLabel: t("save"),
        onSave: save,
        deleteAction: fact
          ? {
              ariaLabel: t("factDelete"),
              confirm: {
                title: t("factDeleteTitle"),
                message: fact.content,
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
                destructive: true,
              },
              onDelete: async () => {
                await adminApi.deleteFact(device.id, fact.id);
                onSaved();
              },
            }
          : undefined,
      }}
    >
      <div className="flex flex-col gap-3">
        <VeitField label={t("fact")} htmlFor="fact-content" hint={t("factHint")}>
          <VeitInput id="fact-content" value={draft.content} disabled={saving} onChange={(e) => setDraft({ content: e.target.value })} />
        </VeitField>
        {error && <NoticeBadge notice={{ ok: false, text: error }} />}
      </div>
    </VeitOverlayEditDialog>
  );
}

/** One message of the history; deleting sits next to closing. */
function MessageDialog({
  device,
  message,
  onClose,
  onDeleted,
}: {
  device: Device;
  message: Message | null;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const { t, formatTime } = useI18n();
  const binding = useManagedOverlayDialogBinding(message !== null, onClose);
  return (
    <VeitOverlayDialog
      binding={binding}
      title={message?.role === "user" ? t("messageUserTitle") : t("messageAssistantTitle")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="sm"
    >
      {message && (
        <div className="flex flex-col gap-4">
          <p className="m-0 whitespace-pre-wrap text-sm">{message.content}</p>
          <p className="m-0 text-xs text-muted-foreground">{formatTime(message.created_at)}</p>
          <div className={VEIT_SETTINGS_ACTIONS}>
            <VeitDeleteButton
              deleteConfirm={{
                title: t("deleteMessageTitle"),
                message: t("deleteMessageText"),
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
                destructive: true,
              }}
              onClick={async () => {
                await adminApi.deleteMessage(device.id, message.id);
                onDeleted();
                onClose();
              }}
            >
              {t("delete")}
            </VeitDeleteButton>
            <VeitButton variant="secondary" onClick={onClose}>
              {t("close")}
            </VeitButton>
          </div>
        </div>
      )}
    </VeitOverlayDialog>
  );
}

function DeviceMemory({ device, version }: { device: Device; version: number }) {
  const { t, formatTime } = useI18n();
  const [memory, setMemory] = useState<{ facts: Fact[]; messages: Message[] }>({ facts: [], messages: [] });
  const [editing, setEditing] = useState<Fact | "new" | null>(null);
  const [message, setMessage] = useState<Message | null>(null);
  const reload = useCallback(() => {
    void adminApi.memory(device.id).then(setMemory);
  }, [device.id]);
  useEffect(reload, [reload, version]);

  const clearConfirm = (title: string) => ({
    title,
    message: t("cannotUndo"),
    confirmLabel: t("delete"),
    cancelLabel: t("cancel"),
    destructive: true,
  });

  return (
    <>
      <VeitSettingsSection
        title={t("factsTitle")}
        description={t("factsHint")}
        actions={
          <VeitButton variant="secondary" onClick={() => setEditing("new")}>
            {t("add")}
          </VeitButton>
        }
      >
        <div className="flex flex-col gap-3">
          {memory.facts.length === 0 ? (
            <VeitEmptyState embedded title={t("noFacts")} />
          ) : (
            <VeitList>
              {memory.facts.map((f) => (
                <VeitListItem key={f.id} title={f.content} onClick={() => setEditing(f)} trailing={chevron} />
              ))}
            </VeitList>
          )}
          {memory.facts.length > 0 && (
            <div className={VEIT_SETTINGS_ACTIONS}>
              <VeitDeleteButton
                deleteConfirm={clearConfirm(t("clearFactsTitle"))}
                onClick={async () => {
                  await adminApi.clearMemory(device.id, "facts");
                  reload();
                }}
              >
                {t("clearFacts")}
              </VeitDeleteButton>
            </div>
          )}
        </div>
      </VeitSettingsSection>
      <FactEditDialog
        open={editing !== null}
        device={device}
        fact={editing === "new" ? null : editing}
        onClose={() => setEditing(null)}
        onSaved={reload}
      />
      <VeitSettingsSection title={t("historyTitle")} description={t("historyHint")}>
        <div className="flex flex-col gap-3">
          {memory.messages.length === 0 ? (
            <VeitEmptyState embedded title={t("noHistory")} />
          ) : (
            <VeitList>
              {memory.messages.map((m) => (
                <VeitListItem
                  key={m.id}
                  title={<span className="line-clamp-2">{m.content}</span>}
                  meta={`${m.role === "user" ? t("roleUser") : t("roleAssistant")} · ${formatTime(m.created_at)}`}
                  trailing={chevron}
                  onClick={() => setMessage(m)}
                />
              ))}
            </VeitList>
          )}
          {memory.messages.length > 0 && (
            <div className={VEIT_SETTINGS_ACTIONS}>
              <VeitDeleteButton
                deleteConfirm={clearConfirm(t("clearHistoryTitle"))}
                onClick={async () => {
                  await adminApi.clearMemory(device.id, "messages");
                  reload();
                }}
              >
                {t("clearHistory")}
              </VeitDeleteButton>
            </div>
          )}
        </div>
      </VeitSettingsSection>
      <MessageDialog device={device} message={message} onClose={() => setMessage(null)} onDeleted={reload} />
    </>
  );
}

export function DevicesPage() {
  const { t } = useI18n();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [voices, setVoices] = useState<Voice[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<Device | "new" | null>(null);
  const [memoryVersion, setMemoryVersion] = useState(0);
  const [notice, setNotice] = useState<Notice>(null);

  const reload = useCallback(async (select?: string) => {
    const list = await adminApi.devices();
    setDevices(list);
    setSelected((s) => select ?? (s && list.some((d) => d.id === s) ? s : (list[0]?.id ?? null)));
  }, []);

  useEffect(() => {
    void reload();
    void adminApi.voices().then(setVoices).catch(() => setVoices([]));
    void adminApi.providers().then(setProviders);
  }, [reload]);

  const device = devices?.find((d) => d.id === selected) ?? null;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      <div className="flex flex-wrap items-center gap-3">
        {devices && devices.length > 0 && (
          <VeitTabBar ariaLabel={t("devicesTabs")} value={selected ?? ""} onChange={setSelected} items={devices.map((d) => ({ id: d.id, label: d.name }))} />
        )}
        <VeitButton variant="secondary" onClick={() => setEditing("new")}>
          {t("addDevice")}
        </VeitButton>
      </div>
      <NoticeBadge notice={notice} />
      {devices?.length === 0 && <VeitEmptyState title={t("noDeviceTitle")} description={t("noDeviceText")} />}
      {device && (
        <>
          <VeitSettingsSection title={t("deviceSection")}>
            <DeviceSummary device={device} voices={voices} providers={providers} onClick={() => setEditing(device)} />
          </VeitSettingsSection>
          <DeviceTest key={`test-${device.id}`} device={device} onMemoryChanged={() => setMemoryVersion((v) => v + 1)} />
          <DeviceMemory key={`memory-${device.id}`} device={device} version={memoryVersion} />
        </>
      )}
      <DeviceEditDialog
        open={editing !== null}
        device={editing === "new" ? null : editing}
        voices={voices}
        providers={providers}
        onClose={() => setEditing(null)}
        onSaved={(saved) => {
          setNotice(null);
          void reload(saved.id);
        }}
        onDeleted={() => {
          setEditing(null);
          void reload();
        }}
        onError={(text) => setNotice({ ok: false, text })}
      />
    </div>
  );
}
