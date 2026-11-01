import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VeitField,
  VeitInput,
  VeitList,
  VeitListItem,
  VeitPasswordField,
  VeitSettingsSection,
  VeitSettingsToggleRow,
  VeitStatusBadge,
} from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText } from "../../api";

type Values = Record<string, string | boolean>;

type Field =
  | { key: string; label: string; kind: "text"; hint?: string }
  | { key: string; label: string; kind: "toggle"; hint?: string }
  /** Write-only: the API only says whether it is set (`<key>_set`). */
  | { key: string; label: string; kind: "password"; hint?: string };

type Group = { id: string; title: string; description: string; fields: Field[]; summary: (v: Values) => string };

const GROUPS: Group[] = [
  {
    id: "accounts",
    title: "Konten",
    description: "Neue Konten haben keinen Adminzugang; Admins bekommen das Recht in der Datenbank (users.is_admin).",
    fields: [{ key: "registration_open", label: "Registrierung offen", kind: "toggle" }],
    summary: (v) => (v.registration_open === "1" ? "Registrierung offen" : "Registrierung geschlossen"),
  },
  {
    id: "smtp",
    title: "E-Mail (SMTP)",
    description: "Für Passwort-Zurücksetzen, E-Mail-Bestätigung und Codes bei der Anmeldung.",
    fields: [
      { key: "smtp_host", label: "Server", kind: "text" },
      { key: "smtp_port", label: "Port", kind: "text" },
      { key: "smtp_secure", label: "TLS ab Verbindungsbeginn (Port 465)", kind: "toggle" },
      { key: "smtp_user", label: "Benutzer", kind: "text" },
      { key: "smtp_password", label: "Passwort", kind: "password" },
      { key: "smtp_from", label: "Absender", kind: "text", hint: "z. B. Sprachassistent <assistent@example.de>" },
    ],
    summary: (v) => (v.smtp_host ? `${v.smtp_host}:${v.smtp_port} · ${v.smtp_from || "kein Absender"}` : "nicht eingerichtet"),
  },
  {
    id: "speech",
    title: "Spracherkennung und -ausgabe",
    description: "Wird nach einem Neustart des Sprachservers wirksam (pm2 restart sprachassistent-voice).",
    fields: [
      { key: "whisper_model", label: "Whisper-Modell", kind: "text", hint: "Pfad relativ zum Projekt, z. B. models/whisper/ggml-small-q5_1.bin" },
      { key: "whisper_language", label: "Sprache", kind: "text", hint: "de, en, …" },
      { key: "whisper_threads", label: "Threads Spracherkennung", kind: "text" },
      { key: "whisper_beam_size", label: "Beam-Größe", kind: "text", hint: "1 = am schnellsten" },
      { key: "tts_threads", label: "Threads Sprachausgabe", kind: "text" },
    ],
    summary: (v) => `${String(v.whisper_model).split("/").pop()} · ${v.whisper_language} · ${v.whisper_threads} Threads`,
  },
];

function draftOf(group: Group, values: Values): Record<string, string> {
  return Object.fromEntries(group.fields.map((f) => [f.key, f.kind === "password" ? "" : String(values[f.key] ?? "")]));
}

function SettingsEditDialog({
  open,
  group,
  values,
  onClose,
  onSaved,
}: {
  open: boolean;
  group: Group;
  values: Values;
  onClose: () => void;
  onSaved: (values: Values) => void;
}) {
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(() => draftOf(group, values), [group, values]);
  const [draft, setDraft] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (key: string, value: string) => setDraft((d) => ({ ...d, [key]: value }));
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
      onSaved(await adminApi.saveSettings(draft));
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
      title={group.title}
      closeAriaLabel="Schließen"
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{ busy: saving, dirty, formBaseline, cancelLabel: "Abbrechen", saveLabel: "Speichern", onSave: save }}
    >
      <div className="flex flex-col gap-4">
        <p className="m-0 text-sm text-muted-foreground">{group.description}</p>
        {group.fields.map((f) =>
          f.kind === "toggle" ? (
            <VeitSettingsToggleRow
              key={f.key}
              variant="embed"
              label={f.label}
              checked={draft[f.key] === "1"}
              disabled={saving}
              onCheckedChange={(on) => set(f.key, on ? "1" : "0")}
            />
          ) : (
            <VeitField
              key={f.key}
              label={f.label}
              htmlFor={f.key}
              hint={f.kind === "password" && values[`${f.key}_set`] ? "Gesetzt; leer lassen, um es zu behalten." : f.hint}
            >
              {f.kind === "password" ? (
                <VeitPasswordField
                  id={f.key}
                  autoComplete="off"
                  value={draft[f.key]}
                  disabled={saving}
                  onChange={(e) => set(f.key, e.target.value)}
                  showPasswordLabel="Passwort zeigen"
                  hidePasswordLabel="Passwort verbergen"
                />
              ) : (
                <VeitInput id={f.key} value={draft[f.key]} disabled={saving} onChange={(e) => set(f.key, e.target.value)} />
              )}
            </VeitField>
          ),
        )}
        {error && (
          <VeitStatusBadge variant="destructive" shape="rounded">
            {error}
          </VeitStatusBadge>
        )}
      </div>
    </VeitOverlayEditDialog>
  );
}

export function SettingsPage() {
  const [values, setValues] = useState<Values | null>(null);
  const [editing, setEditing] = useState<Group | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(() => {
    adminApi.settings().then(setValues, (e) => setError(errorText(e)));
  }, []);
  useEffect(reload, [reload]);

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      {error && (
        <VeitStatusBadge variant="destructive" shape="rounded">
          {error}
        </VeitStatusBadge>
      )}
      {values && (
        <VeitSettingsSection title="Einstellungen" description="Klicken zum Bearbeiten.">
          <VeitList>
            {GROUPS.map((g) => (
              <VeitListItem
                key={g.id}
                title={g.title}
                meta={g.summary(values)}
                trailing={<ChevronRight className="h-4 w-4 text-muted-foreground" aria-hidden />}
                onClick={() => setEditing(g)}
              />
            ))}
          </VeitList>
        </VeitSettingsSection>
      )}
      {values && editing && (
        <SettingsEditDialog
          open
          group={editing}
          values={values}
          onClose={() => setEditing(null)}
          onSaved={(saved) => setValues(saved)}
        />
      )}
    </div>
  );
}
