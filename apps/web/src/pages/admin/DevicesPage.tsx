import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VeitButton,
  VeitDeleteButton,
  VeitDeleteIconButton,
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
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, speakUrl, type Device, type Fact, type Message, type Provider, type Voice } from "../../api";
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

/** Overview of a device; a click opens the edit dialog. */
function DeviceSummary({ device, voices, providers, onClick }: { device: Device; voices: Voice[]; providers: Provider[]; onClick: () => void }) {
  const voice = voices.find((v) => v.id === device.voice)?.label ?? device.voice;
  const provider = providers.find((p) => p.id === device.provider_id)?.name ?? "Standard";
  return (
    <VeitButton
      variant="ghost"
      type="button"
      className="flex w-full items-center gap-3 px-4 py-3 text-left"
      onClick={onClick}
    >
      <div className="min-w-0 flex-1 space-y-1">
        <p className="truncate text-base font-semibold">{device.name}</p>
        <p className="truncate text-sm text-muted-foreground">
          Stimme {voice} · Tempo {device.speed} · KI {provider} · Gedächtnis {device.memory_extraction ? "an" : "aus"}
        </p>
        <p className="line-clamp-2 text-sm text-muted-foreground">{device.persona || "Keine Persönlichkeit gesetzt"}</p>
        <p className="text-xs text-muted-foreground">Geräte-ID: {device.id}</p>
      </div>
      <ChevronRight className="h-5 w-5 shrink-0 text-muted-foreground" aria-hidden />
    </VeitButton>
  );
}

/** Text chat and voice preview with the saved settings of the device. */
function DeviceTest({ device, onMemoryChanged }: { device: Device; onMemoryChanged: () => void }) {
  const [text, setText] = useState("Hallo, wer bist du?");
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
        setNotice({ ok: true, text: `${r.reply} (${r.ms} ms)` });
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
    <VeitSettingsSection title="Testen" description="Antworten gehen ins Gedächtnis des Geräts, wie bei einem echten Gespräch.">
      <div className="flex flex-col gap-4">
        <VeitField label="Text" htmlFor="device-test">
          <VeitTextarea id="device-test" rows={2} value={text} onChange={(e) => setText(e.target.value)} />
        </VeitField>
        <div className="flex flex-wrap gap-2">
          <VeitButton type="button" disabled={busy || !text.trim()} onClick={() => void run("chat")}>
            Fragen
          </VeitButton>
          <VeitButton type="button" variant="secondary" disabled={busy || !text.trim()} onClick={() => void run("speak")}>
            Vorlesen
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
      title={fact ? "Fakt bearbeiten" : "Fakt hinzufügen"}
      closeAriaLabel="Schließen"
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveDisabled: !draft.content.trim(),
        cancelLabel: "Abbrechen",
        saveLabel: "Speichern",
        onSave: save,
        deleteAction: fact
          ? {
              ariaLabel: "Fakt löschen",
              onDelete: async () => {
                await adminApi.deleteFact(device.id, fact.id);
                onSaved();
              },
            }
          : undefined,
      }}
    >
      <div className="flex flex-col gap-3">
        <VeitField label="Fakt" htmlFor="fact-content" hint="Kurz, in der dritten Person, z. B. „Trinkt Kaffee schwarz“.">
          <VeitInput id="fact-content" value={draft.content} disabled={saving} onChange={(e) => setDraft({ content: e.target.value })} />
        </VeitField>
        {error && <NoticeBadge notice={{ ok: false, text: error }} />}
      </div>
    </VeitOverlayEditDialog>
  );
}

function DeviceMemory({ device, version }: { device: Device; version: number }) {
  const [memory, setMemory] = useState<{ facts: Fact[]; messages: Message[] }>({ facts: [], messages: [] });
  const [editing, setEditing] = useState<Fact | "new" | null>(null);
  const reload = useCallback(() => {
    void adminApi.memory(device.id).then(setMemory);
  }, [device.id]);
  useEffect(reload, [reload, version]);

  const clearConfirm = (what: string) => ({
    title: `${what} löschen?`,
    message: "Das lässt sich nicht rückgängig machen.",
    confirmLabel: "Löschen",
    cancelLabel: "Abbrechen",
    destructive: true,
  });

  return (
    <>
      <VeitSettingsSection
        title="Gedächtnis: Fakten"
        description="Fließen in jeden Prompt dieses Geräts ein. Klicken zum Bearbeiten."
        actions={
          <VeitButton type="button" variant="secondary" className="btn-sm" onClick={() => setEditing("new")}>
            Hinzufügen
          </VeitButton>
        }
      >
        <div className="flex flex-col gap-3">
          {memory.facts.length === 0 ? (
            <VeitEmptyState embedded title="Noch keine Fakten" />
          ) : (
            <VeitList>
              {memory.facts.map((f) => (
                <VeitListItem key={f.id} title={f.content} onClick={() => setEditing(f)} trailing={<ChevronRight className="h-4 w-4 text-muted-foreground" aria-hidden />} />
              ))}
            </VeitList>
          )}
          {memory.facts.length > 0 && (
            <div>
              <VeitDeleteButton
                type="button"
                deleteConfirm={clearConfirm("Alle Fakten")}
                onClick={async () => {
                  await adminApi.clearMemory(device.id, "facts");
                  reload();
                }}
              >
                Alle Fakten löschen
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
      <VeitSettingsSection title="Gesprächsverlauf" description="Die letzten 50 Nachrichten.">
        <div className="flex flex-col gap-3">
          {memory.messages.length === 0 ? (
            <VeitEmptyState embedded title="Noch keine Gespräche" />
          ) : (
            <VeitList>
              {memory.messages.map((m) => (
                <VeitListItem
                  key={m.id}
                  title={m.content}
                  meta={`${m.role === "user" ? "Nutzer" : "Assistent"} · ${new Date(m.created_at).toLocaleString("de-DE")}`}
                  trailing={
                    <VeitDeleteIconButton
                      label="Nachricht löschen"
                      onClick={async () => {
                        await adminApi.deleteMessage(device.id, m.id);
                        reload();
                      }}
                    />
                  }
                />
              ))}
            </VeitList>
          )}
          {memory.messages.length > 0 && (
            <div>
              <VeitDeleteButton
                type="button"
                deleteConfirm={clearConfirm("Verlauf")}
                onClick={async () => {
                  await adminApi.clearMemory(device.id, "messages");
                  reload();
                }}
              >
                Verlauf löschen
              </VeitDeleteButton>
            </div>
          )}
        </div>
      </VeitSettingsSection>
    </>
  );
}

export function DevicesPage() {
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
          <VeitTabBar ariaLabel="Geräte" value={selected ?? ""} onChange={setSelected} items={devices.map((d) => ({ id: d.id, label: d.name }))} />
        )}
        <VeitButton type="button" variant="secondary" className="btn-sm" onClick={() => setEditing("new")}>
          Gerät hinzufügen
        </VeitButton>
      </div>
      <NoticeBadge notice={notice} />
      {devices?.length === 0 && <VeitEmptyState title="Noch kein Gerät" description="Lege ein Gerät für deinen ESP an." />}
      {device && (
        <>
          <VeitSettingsSection title="Gerät">
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
