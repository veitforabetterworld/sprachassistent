import { useEffect, useMemo, useState } from "react";
import { VEIT_SETTINGS_ACTIONS, VeitAlert, VeitDeleteButton, VeitField, VeitInput, VeitSelect, VeitSettingsToggleRow, VeitTextarea } from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type Device, type DeviceInput, type Voice } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

const DEFAULT_VOICE = "vits-piper-de_DE-thorsten-medium"; // i18n-ignore: voice id
const DEFAULT_PROVIDER = "default";
/** Same limit as CONTEXT_CHARS in src/context.rs. */
const CONTEXT_CHARS = 500;

/** Text fields are edited as strings and converted on save. The context (memory) is saved with the rest. */
type Draft = Omit<DeviceInput, "speed" | "max_iterations"> & {
  speed: string;
  max_iterations: string;
  context: string;
};

function toDraft(d: DeviceInput, context: string): Draft {
  return {
    ...d,
    speed: String(d.speed),
    max_iterations: String(d.max_iterations),
    context,
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
  onHistoryCleared?: () => void;
};

export function DeviceEditDialog({ open, device, voices, providers, onClose, onSaved, onDeleted, onHistoryCleared }: Props) {
  const { t, tf } = useI18n();
  const binding = useManagedOverlayDialogBinding(open, onClose);
  /** The stored context of the device; null while it loads (new devices have none). */
  const [savedContext, setSavedContext] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    setError(null);
    if (!device) {
      setSavedContext("");
      return;
    }
    setSavedContext(null);
    let current = true;
    adminApi.memory(device.id).then(
      (memory) => current && setSavedContext(memory.context),
      (e: unknown) => current && setError(errorText(e)),
    );
    return () => {
      current = false;
    };
  }, [open, device]);
  const initial = useMemo(
    () =>
      toDraft(
        device ?? {
          name: t("newDeviceName"),
          voice: voices[0]?.id ?? DEFAULT_VOICE,
          speed: 1,
          persona: t("newDevicePersona"),
          provider_id: null,
          max_iterations: 6,
          busy_phrase: t("defaultBusyPhrase"),
          still_busy_phrase: t("defaultStillBusyPhrase"),
          tools_enabled: true,
        },
        savedContext ?? "",
      ),
    [device, voices, t, savedContext],
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
    const { context, ...fields } = draft;
    const input: DeviceInput = {
      ...fields,
      speed: Number(draft.speed.replace(",", ".")) || 1,
      max_iterations: Number(draft.max_iterations) || 6,
    };
    setSaving(true);
    setError(null);
    try {
      const saved = device ? await adminApi.updateDevice(device.id, input) : await adminApi.createDevice(input);
      if (device && savedContext !== null && context.trim() !== savedContext) await adminApi.setContext(device.id, context);
      onSaved(saved);
    } catch (e) {
      setError(errorText(e));
      throw e;
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
        {device && (
          <MemoryFields
            deviceId={device.id}
            context={draft.context}
            loading={savedContext === null}
            disabled={saving}
            onContextChange={(context) => set("context", context)}
            onHistoryCleared={onHistoryCleared}
          />
        )}
        {error && <VeitAlert>{error}</VeitAlert>}
      </div>
    </VeitOverlayEditDialog>
  );
}

/**
 * The chatbot's memory: the context is part of the draft and saved with the dialog. Deleting the history is an
 * action of its own (with confirmation), like deleting the chatbot.
 */
function MemoryFields({
  deviceId,
  context,
  loading,
  disabled,
  onContextChange,
  onHistoryCleared,
}: {
  deviceId: string;
  context: string;
  loading: boolean;
  disabled: boolean;
  onContextChange: (context: string) => void;
  onHistoryCleared?: () => void;
}) {
  const { t, tf } = useI18n();
  return (
    <div className="flex flex-col gap-3 border-t border-border pt-4">
      <VeitField
        label={t("contextTitle")}
        htmlFor="device-context"
        hint={`${tf("contextHint", { max: CONTEXT_CHARS })} ${tf("contextCount", { count: context.length, max: CONTEXT_CHARS })}`}
      >
        <VeitTextarea
          id="device-context"
          rows={5}
          maxLength={CONTEXT_CHARS}
          value={context}
          disabled={disabled || loading}
          onChange={(e) => onContextChange(e.target.value)}
        />
      </VeitField>
      <div className={VEIT_SETTINGS_ACTIONS}>
        <VeitDeleteButton
          disabled={disabled}
          deleteConfirm={{
            title: t("clearHistoryTitle"),
            message: t("cannotUndo"),
            confirmLabel: t("delete"),
            cancelLabel: t("cancel"),
            destructive: true,
          }}
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
