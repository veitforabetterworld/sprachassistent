import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VeitField,
  VeitInput,
  VeitList,
  VeitListItem,
  VeitSecretField,
  VeitSettingsSection,
  VeitSettingsToggleRow,
  VeitStatusBadge,
} from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText } from "../../api";
import type { MessageKey } from "../../i18n/de";
import { useI18n } from "../../i18n/I18nProvider";

type Values = Record<string, string | boolean>;
type Tf = (key: MessageKey, values: Record<string, string | number>) => string;
type T = (key: MessageKey) => string;

type Field =
  | { key: string; label: MessageKey; kind: "text"; hint?: MessageKey }
  | { key: string; label: MessageKey; kind: "toggle"; hint?: MessageKey }
  /** Write-only: the API only says whether it is set (`<key>_set`). */
  | { key: string; label: MessageKey; kind: "password"; hint?: MessageKey };

type Group = { id: string; title: MessageKey; description: MessageKey; fields: Field[]; summary: (v: Values, t: T, tf: Tf) => string };

const GROUPS: Group[] = [
  {
    id: "smtp",
    title: "smtpTitle",
    description: "smtpHint",
    fields: [
      { key: "smtp_host", label: "smtpHost", kind: "text" },
      { key: "smtp_port", label: "smtpPort", kind: "text" },
      { key: "smtp_secure", label: "smtpSecure", kind: "toggle" },
      { key: "smtp_user", label: "smtpUser", kind: "text" },
      { key: "smtp_password", label: "smtpPassword", kind: "password" },
      { key: "smtp_from", label: "smtpFrom", kind: "text", hint: "smtpFromHint" },
    ],
    summary: (v, t, tf) =>
      v.smtp_host ? tf("smtpSummary", { host: String(v.smtp_host), port: String(v.smtp_port), from: String(v.smtp_from || t("smtpNoFrom")) }) : t("notConfigured"),
  },
  {
    id: "tools",
    title: "toolsTitle",
    description: "toolsHint",
    fields: [
      { key: "google_oauth_client_id", label: "googleClientId", kind: "text", hint: "googleOAuthHint" },
      { key: "google_oauth_client_secret", label: "googleClientSecret", kind: "password" },
    ],
    summary: (v, t, tf) =>
      tf("toolsSummary", {
        google: v.google_oauth_client_id ? t("on") : t("off"),
      }),
  },
  {
    id: "speech",
    title: "speechTitle",
    description: "speechHint",
    fields: [
      { key: "whisper_model", label: "whisperModel", kind: "text", hint: "whisperModelHint" },
      { key: "whisper_language", label: "whisperLanguage", kind: "text", hint: "whisperLanguageHint" },
      { key: "whisper_threads", label: "whisperThreads", kind: "text" },
      { key: "whisper_beam_size", label: "whisperBeam", kind: "text", hint: "whisperBeamHint" },
      { key: "tts_threads", label: "ttsThreads", kind: "text" },
    ],
    summary: (v, _t, tf) =>
      tf("speechSummary", { model: String(v.whisper_model).split("/").pop() ?? "", language: String(v.whisper_language), threads: String(v.whisper_threads) }),
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
  const { t } = useI18n();
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
      title={t(group.title)}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{ busy: saving, dirty, formBaseline, cancelLabel: t("cancel"), saveLabel: t("save"), onSave: save }}
    >
      <div className="flex flex-col gap-4">
        <p className="m-0 text-sm text-muted-foreground">{t(group.description)}</p>
        {group.fields.map((f) =>
          f.kind === "toggle" ? (
            <VeitSettingsToggleRow
              key={f.key}
              variant="embed"
              label={t(f.label)}
              checked={draft[f.key] === "1"}
              disabled={saving}
              onCheckedChange={(on) => set(f.key, on ? "1" : "0")}
            />
          ) : (
            <VeitField
              key={f.key}
              label={t(f.label)}
              htmlFor={f.key}
              hint={f.kind === "password" && values[`${f.key}_set`] ? t("secretSet") : f.hint && t(f.hint)}
            >
              {f.kind === "password" ? (
                <VeitSecretField
                  id={f.key}
                  value={draft[f.key]}
                  onChange={(v) => set(f.key, v)}
                  stored={values[`${f.key}_set`] === true}
                  onReveal={() => adminApi.settingSecret(f.key)}
                  onRevealError={(e) => setError(errorText(e))}
                  showLabel={t("showPassword")}
                  hideLabel={t("hidePassword")}
                  copyLabel={t("copy")}
                  copiedLabel={t("copied")}
                  disabled={saving}
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
  const { t, tf } = useI18n();
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
        <VeitSettingsSection title={t("settingsTitle")}>
          <VeitList>
            {GROUPS.map((g) => (
              <VeitListItem
                key={g.id}
                title={t(g.title)}
                meta={g.summary(values, t, tf)}
                trailing={<ChevronRight className="size-4 text-muted-foreground" aria-hidden />}
                onClick={() => setEditing(g)}
              />
            ))}
          </VeitList>
        </VeitSettingsSection>
      )}
      {/* Stays mounted; the overlay keeps its place in the history while open toggles. */}
      <SettingsEditDialog
        open={values !== null && editing !== null}
        group={editing ?? GROUPS[0]}
        values={values ?? {}}
        onClose={() => setEditing(null)}
        onSaved={(saved) => setValues(saved)}
      />
    </div>
  );
}
