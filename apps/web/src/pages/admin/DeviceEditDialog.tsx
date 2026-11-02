import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import { VEIT_SETTINGS_ACTIONS, VeitButton, VeitDeleteButton, VeitEmptyState, VeitField, VeitInput, VeitList, VeitListItem, VeitSelect, VeitSettingsToggleRow, VeitTextarea } from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type Device, type DeviceInput, type Fact, type Voice } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

const DEFAULT_VOICE = "vits-piper-de_DE-thorsten-medium"; // i18n-ignore: voice id
const DEFAULT_PROVIDER = "default";

/** Text fields are edited as strings and converted on save. */
type Draft = Omit<DeviceInput, "speed" | "history_messages" | "max_iterations"> & {
  speed: string;
  history_messages: string;
  max_iterations: string;
};

function toDraft(d: DeviceInput): Draft {
  return {
    ...d,
    speed: String(d.speed),
    history_messages: String(d.history_messages),
    max_iterations: String(d.max_iterations),
  };
}

type Props = {
  open: boolean;
  /** null creates a new device. */
  device: Device | null;
  voices: Voice[];
  providers: { id: string; name: string }[];
  onClose: () => void;
  onSaved: (device: Device) => void;
  onDeleted: () => void;
  onError: (message: string) => void;
  onHistoryCleared?: () => void;
};

export function DeviceEditDialog({ open, device, voices, providers, onClose, onSaved, onDeleted, onError, onHistoryCleared }: Props) {
  const { t, tf } = useI18n();
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(
    () =>
      toDraft(
        device ?? {
          name: t("newDeviceName"),
          voice: voices[0]?.id ?? DEFAULT_VOICE,
          speed: 1,
          persona: t("newDevicePersona"),
          provider_id: null,
          history_messages: 12,
          memory_extraction: true,
          max_iterations: 6,
          busy_phrase: t("defaultBusyPhrase"),
          still_busy_phrase: t("defaultStillBusyPhrase"),
          tools_enabled: true,
        },
      ),
    [device, voices, t],
  );
  const [draft, setDraft] = useState<Draft>(initial);
  const [saving, setSaving] = useState(false);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((d) => ({ ...d, [key]: value }));

  useEffect(() => {
    if (open) setDraft(initial);
  }, [open, initial]);

  const { dirty, formBaseline } = useDialogFormBaseline({
    draft,
    resolveBaseline: () => initial,
    resetDeps: [open, initial],
    track: open,
  });

  const save = async () => {
    const input: DeviceInput = {
      ...draft,
      speed: Number(draft.speed.replace(",", ".")) || 1,
      history_messages: Number(draft.history_messages) || 0,
      max_iterations: Number(draft.max_iterations) || 6,
    };
    setSaving(true);
    try {
      onSaved(device ? await adminApi.updateDevice(device.id, input) : await adminApi.createDevice(input));
    } catch (error) {
      onError(errorText(error));
      throw error;
    } finally {
      setSaving(false);
    }
  };

  const voiceOptions = [
    ...voices.map((v) => ({ value: v.id, label: v.label })),
    ...(voices.some((v) => v.id === draft.voice) ? [] : [{ value: draft.voice, label: tf("voiceMissing", { voice: draft.voice }) }]),
  ];

  return (
    <VeitOverlayEditDialog
      binding={binding}
      title={device ? tf("editDevice", { name: device.name }) : t("newDevice")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="lg"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveRequiresDirty: device !== null,
        cancelLabel: t("cancel"),
        saveLabel: device ? t("save") : t("create"),
        onSave: save,
        deleteAction: device
          ? {
              ariaLabel: t("deleteDevice"),
              confirm: {
                title: t("deleteDeviceTitle"),
                message: tf("deleteDeviceText", { name: device.name }),
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
                destructive: true,
              },
              onDelete: async () => {
                await adminApi.deleteDevice(device.id);
                onDeleted();
              },
            }
          : undefined,
      }}
    >
      <div className="flex flex-col gap-4">
        <VeitField label={t("name")} htmlFor="device-name">
          <VeitInput id="device-name" value={draft.name} disabled={saving} onChange={(e) => set("name", e.target.value)} />
        </VeitField>
        <VeitField label={t("voice")} htmlFor="device-voice">
          <VeitSelect id="device-voice" value={draft.voice} disabled={saving} onValueChange={(v) => set("voice", v)} options={voiceOptions} />
        </VeitField>
        <VeitField label={t("speed")} htmlFor="device-speed" hint={t("speedHint")}>
          <VeitInput id="device-speed" inputMode="decimal" value={draft.speed} disabled={saving} onChange={(e) => set("speed", e.target.value)} />
        </VeitField>
        <VeitField label={t("persona")} htmlFor="device-persona" hint={t("personaHint")}>
          <VeitTextarea id="device-persona" rows={7} value={draft.persona} disabled={saving} onChange={(e) => set("persona", e.target.value)} />
        </VeitField>
        <VeitField label={t("provider")} htmlFor="device-provider">
          <VeitSelect
            id="device-provider"
            value={draft.provider_id ?? DEFAULT_PROVIDER}
            disabled={saving}
            onValueChange={(v) => set("provider_id", v === DEFAULT_PROVIDER ? null : v)}
            options={[
              { value: DEFAULT_PROVIDER, label: t("providerDefault") },
              ...providers.map((p) => ({ value: p.id, label: tf("providerFirst", { name: p.name }) })),
            ]}
          />
        </VeitField>
        <VeitField label={t("historyMessages")} htmlFor="device-history">
          <VeitInput
            id="device-history"
            inputMode="numeric"
            value={draft.history_messages}
            disabled={saving}
            onChange={(e) => set("history_messages", e.target.value)}
          />
        </VeitField>
        <VeitSettingsToggleRow
          variant="embed"
          label={t("memoryExtraction")}
          description={t("memoryExtractionHint")}
          checked={draft.memory_extraction}
          disabled={saving}
          onCheckedChange={(v) => set("memory_extraction", v)}
        />
        <VeitField label={t("maxIterations")} htmlFor="device-iterations" hint={t("maxIterationsHint")}>
          <VeitInput
            id="device-iterations"
            inputMode="numeric"
            value={draft.max_iterations}
            disabled={saving}
            onChange={(e) => set("max_iterations", e.target.value)}
          />
        </VeitField>
        <VeitField label={t("busyPhrase")} htmlFor="device-busy">
          <VeitInput id="device-busy" value={draft.busy_phrase} disabled={saving} onChange={(e) => set("busy_phrase", e.target.value)} />
        </VeitField>
        <VeitField label={t("stillBusyPhrase")} htmlFor="device-still">
          <VeitInput id="device-still" value={draft.still_busy_phrase} disabled={saving} onChange={(e) => set("still_busy_phrase", e.target.value)} />
        </VeitField>
        <VeitSettingsToggleRow
          variant="embed"
          label={t("toolsEnabled")}
          description={t("toolsEnabledHint")}
          checked={draft.tools_enabled}
          disabled={saving}
          onCheckedChange={(v) => set("tools_enabled", v)}
        />
        {device && <FactsEditor deviceId={device.id} onHistoryCleared={onHistoryCleared} />}
      </div>
    </VeitOverlayEditDialog>
  );
}

/** Facts live with the chatbot settings. Saving one does not touch the rest of the form. */
function FactsEditor({ deviceId, onHistoryCleared }: { deviceId: string; onHistoryCleared?: () => void }) {
  const { t } = useI18n();
  const [facts, setFacts] = useState<Fact[]>([]);
  const [selected, setSelected] = useState<Fact | null>(null);
  const [content, setContent] = useState("");
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(() => {
    void adminApi.memory(deviceId).then((memory) => setFacts(memory.facts));
  }, [deviceId]);
  useEffect(reload, [reload]);

  const save = async () => {
    const text = content.trim();
    if (!text) return;
    setError(null);
    try {
      if (selected) await adminApi.updateFact(deviceId, selected.id, text);
      else await adminApi.addFact(deviceId, text);
      setSelected(null);
      setContent("");
      reload();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const confirm = (title: string) => ({
    title,
    message: t("cannotUndo"),
    confirmLabel: t("delete"),
    cancelLabel: t("cancel"),
    destructive: true,
  });

  return (
    <div className="flex flex-col gap-3 border-t border-border pt-4">
      <div>
        <p className="m-0 text-sm font-medium">{t("factsTitle")}</p>
        <p className="m-0 text-sm text-muted-foreground">{t("factsHint")}</p>
      </div>
      {facts.length === 0 ? (
        <VeitEmptyState embedded title={t("noFacts")} />
      ) : (
        <VeitList>
          {facts.map((fact) => (
            <VeitListItem
              key={fact.id}
              title={fact.content}
              meta={fact.origin === "user" ? t("factOriginUser") : undefined}
              trailing={<ChevronRight className="size-4 text-muted-foreground" aria-hidden />}
              onClick={() => {
                setSelected(fact);
                setContent(fact.content);
              }}
            />
          ))}
        </VeitList>
      )}
      <VeitField label={selected ? t("factEdit") : t("factAdd")} htmlFor="fact-content" hint={t("factHint")}>
        <VeitInput id="fact-content" value={content} onChange={(e) => setContent(e.target.value)} />
      </VeitField>
      {error && <p className="m-0 text-sm text-destructive">{error}</p>}
      <div className={VEIT_SETTINGS_ACTIONS}>
        <VeitButton type="button" disabled={!content.trim()} onClick={() => void save()}>
          {selected ? t("save") : t("add")}
        </VeitButton>
        {selected && (
          <VeitDeleteButton
            deleteConfirm={{ ...confirm(t("factDeleteTitle")), message: selected.content }}
            onClick={async () => {
              await adminApi.deleteFact(deviceId, selected.id);
              setSelected(null);
              setContent("");
              reload();
            }}
          >
            {t("factDelete")}
          </VeitDeleteButton>
        )}
        {facts.length > 0 && (
          <VeitDeleteButton
            deleteConfirm={confirm(t("clearFactsTitle"))}
            onClick={async () => {
              await adminApi.clearMemory(deviceId, "facts");
              setSelected(null);
              setContent("");
              reload();
            }}
          >
            {t("clearFacts")}
          </VeitDeleteButton>
        )}
        <VeitDeleteButton
          deleteConfirm={confirm(t("clearHistoryTitle"))}
          onClick={async () => {
            await adminApi.clearMemory(deviceId, "messages");
            onHistoryCleared?.();
          }}
        >
          {t("clearHistory")}
        </VeitDeleteButton>
      </div>
    </div>
  );
}
