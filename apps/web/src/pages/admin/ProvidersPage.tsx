import { useCallback, useEffect, useMemo, useState } from "react";
import {
  VeitButton,
  VeitField,
  VeitInput,
  VeitList,
  VeitListItem,
  VeitPasswordField,
  VeitSelect,
  VeitSettingsSection,
  VeitSettingsToggleRow,
  VeitStatusBadge,
} from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type Provider, type ProviderInput } from "../../api";

/** OpenAI-compatible endpoints with a free tier (or local). */
const PRESETS: Record<string, ProviderInput & { keyUrl?: string }> = {
  groq: {
    name: "Groq",
    base_url: "https://api.groq.com/openai/v1",
    model: "qwen/qwen3.8-27b",
    api_key: "",
    enabled: true,
    priority: 10,
    keyUrl: "https://console.groq.com/keys",
  },
  gemini: {
    name: "Google Gemini",
    base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
    model: "gemini-2.5-flash-lite",
    api_key: "",
    enabled: true,
    priority: 20,
    keyUrl: "https://aistudio.google.com/apikey",
  },
  openrouter: {
    name: "OpenRouter",
    base_url: "https://openrouter.ai/api/v1",
    model: "meta-llama/llama-3.3-70b-instruct:free",
    api_key: "",
    enabled: true,
    priority: 30,
    keyUrl: "https://openrouter.ai/keys",
  },
  ollama: {
    name: "Ollama (lokal)",
    base_url: "http://127.0.0.1:11434/v1",
    model: "qwen2.5:3b",
    api_key: "",
    enabled: true,
    priority: 40,
  },
};

const KEY_URLS: Record<string, string> = Object.fromEntries(
  Object.values(PRESETS)
    .filter((p) => p.keyUrl)
    .map((p) => [p.base_url, p.keyUrl!]),
);

const isLocal = (url: string) => /^https?:\/\/(localhost|127\.0\.0\.1)[:/]/.test(url);

type Draft = Omit<ProviderInput, "priority"> & { priority: string };

function toDraft(p: ProviderInput): Draft {
  const { name, base_url, model, api_key, enabled, priority } = p;
  return { name, base_url, model, api_key, enabled, priority: String(priority) };
}

function ProviderEditDialog({
  open,
  provider,
  onClose,
  onSaved,
}: {
  open: boolean;
  /** null creates a provider. */
  provider: Provider | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(() => toDraft(provider ? { ...provider, api_key: "" } : PRESETS.groq), [provider]);
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
  const keyUrl = KEY_URLS[draft.base_url];

  const save = async () => {
    const input: ProviderInput = { ...draft, priority: Number(draft.priority) || 0 };
    setSaving(true);
    try {
      if (provider) await adminApi.updateProvider(provider.id, input);
      else await adminApi.createProvider(input);
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
      title={provider ? `${provider.name} bearbeiten` : "Neuer KI-Anbieter"}
      closeAriaLabel="Schließen"
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveRequiresDirty: provider !== null,
        cancelLabel: "Abbrechen",
        saveLabel: provider ? "Speichern" : "Hinzufügen",
        onSave: save,
        deleteAction: provider
          ? {
              ariaLabel: "Anbieter löschen",
              confirm: {
                title: "Anbieter löschen?",
                message: `${provider.name} wird entfernt. Geräte, die ihn bevorzugen, nutzen dann die anderen.`,
                confirmLabel: "Löschen",
                cancelLabel: "Abbrechen",
                destructive: true,
              },
              onDelete: async () => {
                await adminApi.deleteProvider(provider.id);
                onSaved();
              },
            }
          : undefined,
      }}
    >
      <div className="flex flex-col gap-4">
        {!provider && (
          <VeitField label="Vorlage" htmlFor="provider-preset">
            <VeitSelect
              id="provider-preset"
              placeholder="Vorlage wählen"
              disabled={saving}
              onValueChange={(value) => setDraft(toDraft(PRESETS[value]))}
              options={Object.entries(PRESETS).map(([value, p]) => ({ value, label: p.name }))}
            />
          </VeitField>
        )}
        <VeitField label="Name" htmlFor="provider-name">
          <VeitInput id="provider-name" value={draft.name} disabled={saving} onChange={(e) => set("name", e.target.value)} />
        </VeitField>
        <VeitField label="Adresse (OpenAI-kompatibel)" htmlFor="provider-url">
          <VeitInput id="provider-url" value={draft.base_url} disabled={saving} onChange={(e) => set("base_url", e.target.value)} />
        </VeitField>
        <VeitField label="Modell" htmlFor="provider-model">
          <VeitInput id="provider-model" value={draft.model} disabled={saving} onChange={(e) => set("model", e.target.value)} />
        </VeitField>
        <VeitField
          label="API-Key"
          htmlFor="provider-key"
          hint={
            <>
              {provider?.api_key_set ? `Gespeichert (${provider.api_key_hint}); leer lassen, um ihn zu behalten. ` : ""}
              {keyUrl && (
                <a href={keyUrl} target="_blank" rel="noreferrer">
                  Kostenlosen Key holen
                </a>
              )}
            </>
          }
        >
          <VeitPasswordField
            id="provider-key"
            autoComplete="off"
            value={draft.api_key}
            disabled={saving}
            onChange={(e) => set("api_key", e.target.value)}
            showPasswordLabel="Key zeigen"
            hidePasswordLabel="Key verbergen"
          />
        </VeitField>
        <VeitField label="Reihenfolge (kleiner = zuerst)" htmlFor="provider-priority">
          <VeitInput id="provider-priority" inputMode="numeric" value={draft.priority} disabled={saving} onChange={(e) => set("priority", e.target.value)} />
        </VeitField>
        <VeitSettingsToggleRow variant="embed" label="Aktiv" checked={draft.enabled} disabled={saving} onCheckedChange={(v) => set("enabled", v)} />
        {error && (
          <VeitStatusBadge variant="destructive" shape="rounded">
            {error}
          </VeitStatusBadge>
        )}
      </div>
    </VeitOverlayEditDialog>
  );
}

function ProviderRow({ provider, onEdit }: { provider: Provider; onEdit: () => void }) {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const ready = provider.enabled && (provider.api_key_set || isLocal(provider.base_url));

  const test = async () => {
    setBusy(true);
    setResult(null);
    try {
      const r = await adminApi.testProvider(provider.id);
      setResult(r.ok ? { ok: true, text: `„${r.reply}“ in ${r.ms} ms` } : { ok: false, text: r.error ?? "Fehler" });
    } catch (e) {
      setResult({ ok: false, text: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <VeitListItem
      title={provider.name}
      meta={`${provider.model} · Reihenfolge ${provider.priority}`}
      badge={
        <VeitStatusBadge variant={!provider.enabled ? "inactive" : ready ? "success" : "warning"}>
          {!provider.enabled ? "aus" : provider.api_key_set ? `Key ${provider.api_key_hint}` : isLocal(provider.base_url) ? "lokal" : "Key fehlt"}
        </VeitStatusBadge>
      }
      footer={
        result && (
          <VeitStatusBadge variant={result.ok ? "success" : "destructive"} shape="rounded">
            {result.text}
          </VeitStatusBadge>
        )
      }
      actions={
        <div className="flex gap-2">
          <VeitButton type="button" variant="secondary" className="btn-sm" disabled={busy || !ready} onClick={() => void test()}>
            Testen
          </VeitButton>
          <VeitButton type="button" variant="secondary" className="btn-sm" onClick={onEdit}>
            Bearbeiten
          </VeitButton>
        </div>
      }
    />
  );
}

export function ProvidersPage() {
  const [providers, setProviders] = useState<Provider[] | null>(null);
  const [editing, setEditing] = useState<Provider | "new" | null>(null);
  const reload = useCallback(() => {
    void adminApi.providers().then(setProviders);
  }, []);
  useEffect(reload, [reload]);

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      <VeitSettingsSection
        title="KI-Anbieter"
        description="Der Sprachserver fragt die aktiven Anbieter mit Key der Reihe nach, bis einer antwortet; ein Gerät kann einen davon bevorzugen. Groq und Gemini haben kostenlose Kontingente, nur der Key muss eingetragen werden."
        actions={
          <VeitButton type="button" variant="secondary" className="btn-sm" onClick={() => setEditing("new")}>
            Hinzufügen
          </VeitButton>
        }
      >
        <VeitList>
          {providers?.map((p) => <ProviderRow key={p.id} provider={p} onEdit={() => setEditing(p)} />)}
        </VeitList>
      </VeitSettingsSection>
      <ProviderEditDialog
        open={editing !== null}
        provider={editing === "new" ? null : editing}
        onClose={() => setEditing(null)}
        onSaved={() => {
          setEditing(null);
          reload();
        }}
      />
    </div>
  );
}
