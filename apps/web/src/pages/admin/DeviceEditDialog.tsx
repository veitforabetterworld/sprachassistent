import { useEffect, useMemo, useState } from "react";
import { VeitField, VeitInput, VeitSelect, VeitSettingsToggleRow, VeitTextarea } from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type Device, type DeviceInput, type Provider, type Voice } from "../../api";

export const NEW_DEVICE: DeviceInput = {
  name: "Neues Gerät",
  voice: "vits-piper-de_DE-thorsten-medium",
  speed: 1,
  persona: "Du bist ein freundlicher, hilfsbereiter Sprachassistent.",
  provider_id: null,
  history_messages: 12,
  memory_extraction: true,
};

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
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(() => toDraft(device ?? { ...NEW_DEVICE, voice: voices[0]?.id ?? NEW_DEVICE.voice }), [device, voices]);
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
    ...(voices.some((v) => v.id === draft.voice) ? [] : [{ value: draft.voice, label: `${draft.voice} (nicht installiert)` }]),
  ];

  return (
    <VeitOverlayEditDialog
      binding={binding}
      title={device ? `${device.name} bearbeiten` : "Neues Gerät"}
      closeAriaLabel="Schließen"
      variant="responsive"
      size="lg"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveRequiresDirty: device !== null,
        cancelLabel: "Abbrechen",
        saveLabel: device ? "Speichern" : "Anlegen",
        onSave: save,
        deleteAction: device
          ? {
              ariaLabel: "Gerät löschen",
              confirm: {
                title: "Gerät löschen?",
                message: `${device.name} wird mit seinem ganzen Gedächtnis gelöscht. Seine ESP-Schlüssel gelten danach für kein Gerät mehr.`,
                confirmLabel: "Löschen",
                cancelLabel: "Abbrechen",
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
        <VeitField label="Name" htmlFor="device-name">
          <VeitInput id="device-name" value={draft.name} disabled={saving} onChange={(e) => set("name", e.target.value)} />
        </VeitField>
        <VeitField label="Stimme" htmlFor="device-voice">
          <VeitSelect id="device-voice" value={draft.voice} disabled={saving} onValueChange={(v) => set("voice", v)} options={voiceOptions} />
        </VeitField>
        <VeitField label="Sprechtempo" htmlFor="device-speed" hint="1 = normal, 1.2 = schneller, 0.8 = langsamer">
          <VeitInput id="device-speed" inputMode="decimal" value={draft.speed} disabled={saving} onChange={(e) => set("speed", e.target.value)} />
        </VeitField>
        <VeitField label="Persönlichkeit" htmlFor="device-persona" hint="Wer der Assistent ist und wie er spricht.">
          <VeitTextarea id="device-persona" rows={7} value={draft.persona} disabled={saving} onChange={(e) => set("persona", e.target.value)} />
        </VeitField>
        <VeitField label="KI-Anbieter" htmlFor="device-provider">
          <VeitSelect
            id="device-provider"
            value={draft.provider_id ?? "default"}
            disabled={saving}
            onValueChange={(v) => set("provider_id", v === "default" ? null : v)}
            options={[
              { value: "default", label: "Standard (Reihenfolge der KI-Anbieter)" },
              ...providers.map((p) => ({ value: p.id, label: `${p.name} zuerst` })),
            ]}
          />
        </VeitField>
        <VeitField label="Gesprächsverlauf im Prompt (Nachrichten)" htmlFor="device-history">
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
          label="Gedächtnis: Fakten automatisch merken"
          description="Nach jedem Gespräch eine zusätzliche KI-Anfrage."
          checked={draft.memory_extraction}
          disabled={saving}
          onCheckedChange={(v) => set("memory_extraction", v)}
        />
      </div>
    </VeitOverlayEditDialog>
  );
}
