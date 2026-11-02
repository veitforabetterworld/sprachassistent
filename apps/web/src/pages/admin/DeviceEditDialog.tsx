import { useEffect, useMemo, useState } from "react";
import { VeitField, VeitInput, VeitSelect, VeitSettingsToggleRow, VeitTextarea } from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type Device, type DeviceInput, type Provider, type Voice } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

const DEFAULT_VOICE = "vits-piper-de_DE-thorsten-medium"; // i18n-ignore: voice id
const DEFAULT_PROVIDER = "default";

/** Text fields are edited as strings and converted on save. */
type Draft = Omit<DeviceInput, "speed" | "history_messages"> & { speed: string; history_messages: string };

function toDraft(d: DeviceInput): Draft {
  return { ...d, speed: String(d.speed), history_messages: String(d.history_messages) };
}

type Props = {
  open: boolean;
  /** null creates a new device. */
  device: Device | null;
  voices: Voice[];
  providers: Provider[];
  onClose: () => void;
  onSaved: (device: Device) => void;
  onDeleted: () => void;
  onError: (message: string) => void;
};

export function DeviceEditDialog({ open, device, voices, providers, onClose, onSaved, onDeleted, onError }: Props) {
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
      </div>
    </VeitOverlayEditDialog>
  );
}
