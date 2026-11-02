import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, Loader2 } from "lucide-react";
import {
  VeitButton,
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
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type Provider, type ProviderInput } from "../../api";
import type { MessageKey } from "../../i18n/de";
import { useI18n } from "../../i18n/I18nProvider";
import { linkClass } from "../Layout";

/** OpenAI-compatible endpoints with a free tier (or local). `nameKey` names a preset whose name is text. */
const PRESETS: Record<string, ProviderInput & { keyUrl?: string; nameKey?: MessageKey }> = {
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
    name: "Google Gemini", // i18n-ignore: product name
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
    name: "Ollama",
    nameKey: "presetOllama",
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
  const { t, tf } = useI18n();
  const presetName = (key: string) => (PRESETS[key].nameKey ? t(PRESETS[key].nameKey!) : PRESETS[key].name);
  const fromPreset = (key: string) => toDraft({ ...PRESETS[key], name: presetName(key) });
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(() => (provider ? toDraft({ ...provider, api_key: "" }) : toDraft(PRESETS.groq)), [provider]);
  const [draft, setDraft] = useState<Draft>(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((d) => ({ ...d, [key]: value }));
  useEffect(() => {
    if (open) {
      setDraft(initial);
      setError(null);
      setTestResult(null);
    }
  }, [open, initial]);
  // The key stays empty unless a new one is typed; showing the stored one is not a change.
  const { dirty, formBaseline } = useDialogFormBaseline({ draft, resolveBaseline: () => initial, resetDeps: [open, initial], track: open });
  const keyUrl = KEY_URLS[draft.base_url];
  const ready = provider !== null && provider.enabled && (provider.api_key_set || isLocal(provider.base_url));

  const test = async () => {
    if (!provider) return;
    setTesting(true);
    setTestResult(null);
    try {
      const r = await adminApi.testProvider(provider.id);
      setTestResult(r.ok ? { ok: true, text: tf("testReply", { reply: r.reply ?? "", ms: r.ms }) } : { ok: false, text: r.error ?? t("error") });
    } catch (e) {
      setTestResult({ ok: false, text: errorText(e) });
    } finally {
      setTesting(false);
    }
  };

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
      title={provider ? tf("editProvider", { name: provider.name }) : t("newProvider")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="sm"
      disabled={saving}
      blockBackdropClose={saving}
      footerProps={{
        busy: saving,
        dirty,
        formBaseline,
        saveRequiresDirty: provider !== null,
        cancelLabel: t("cancel"),
        saveLabel: provider ? t("save") : t("add"),
        onSave: save,
        deleteAction: provider
          ? {
              ariaLabel: t("deleteProvider"),
              confirm: {
                title: t("deleteProviderTitle"),
                message: tf("deleteProviderText", { name: provider.name }),
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
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
          <VeitField label={t("preset")} htmlFor="provider-preset">
            <VeitSelect
              id="provider-preset"
              placeholder={t("presetChoose")}
              disabled={saving}
              onValueChange={(value) => setDraft(fromPreset(value))}
              options={Object.keys(PRESETS).map((value) => ({ value, label: presetName(value) }))}
            />
          </VeitField>
        )}
        <VeitField label={t("name")} htmlFor="provider-name">
          <VeitInput id="provider-name" value={draft.name} disabled={saving} onChange={(e) => set("name", e.target.value)} />
        </VeitField>
        <VeitField label={t("baseUrl")} htmlFor="provider-url">
          <VeitInput id="provider-url" value={draft.base_url} disabled={saving} onChange={(e) => set("base_url", e.target.value)} />
        </VeitField>
        <VeitField label={t("model")} htmlFor="provider-model">
          <VeitInput id="provider-model" value={draft.model} disabled={saving} onChange={(e) => set("model", e.target.value)} />
        </VeitField>
        <VeitField
          label={t("apiKey")}
          htmlFor="provider-key"
          hint={
            <>
              {provider?.api_key_set ? `${tf("apiKeyStored", { hint: provider.api_key_hint })} ` : ""}
              {keyUrl && (
                <a href={keyUrl} target="_blank" rel="noreferrer" className={linkClass}>
                  {t("getFreeKey")}
                </a>
              )}
            </>
          }
        >
          <VeitSecretField
            key={provider?.id ?? "new"}
            id="provider-key"
            value={draft.api_key}
            onChange={(v) => set("api_key", v)}
            stored={provider?.api_key_set ?? false}
            onReveal={() => adminApi.providerKey(provider!.id)}
            onRevealError={(e) => setError(errorText(e))}
            showLabel={t("showKey")}
            hideLabel={t("hideKey")}
            copyLabel={t("copy")}
            copiedLabel={t("copied")}
            disabled={saving}
          />
        </VeitField>
        <VeitField label={t("priority")} htmlFor="provider-priority">
          <VeitInput id="provider-priority" inputMode="numeric" value={draft.priority} disabled={saving} onChange={(e) => set("priority", e.target.value)} />
        </VeitField>
        <VeitSettingsToggleRow variant="embed" label={t("enabled")} checked={draft.enabled} disabled={saving} onCheckedChange={(v) => set("enabled", v)} />
        {provider && (
          <VeitField label={t("connection")} hint={dirty ? t("connectionHint") : undefined}>
            <div className="flex flex-col items-start gap-2">
              <VeitButton variant="secondary" disabled={saving || testing || !ready} onClick={() => void test()}>
                {testing && <Loader2 className="size-4 animate-spin" aria-hidden />}
                {t("test")}
              </VeitButton>
              {testResult && (
                <VeitStatusBadge variant={testResult.ok ? "success" : "destructive"} shape="rounded">
                  {testResult.text}
                </VeitStatusBadge>
              )}
            </div>
          </VeitField>
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

/** A provider; a click opens the dialog to show, test, edit and delete it. */
function ProviderRow({ provider, onClick }: { provider: Provider; onClick: () => void }) {
  const { t, tf } = useI18n();
  const ready = provider.enabled && (provider.api_key_set || isLocal(provider.base_url));
  return (
    <VeitListItem
      title={provider.name}
      meta={tf("providerMeta", { model: provider.model, priority: provider.priority })}
      badge={
        <VeitStatusBadge variant={!provider.enabled ? "inactive" : ready ? "success" : "warning"}>
          {!provider.enabled
            ? t("providerOff")
            : provider.api_key_set
              ? tf("providerKey", { hint: provider.api_key_hint })
              : isLocal(provider.base_url)
                ? t("providerLocal")
                : t("providerKeyMissing")}
        </VeitStatusBadge>
      }
      trailing={<ChevronRight className="size-4 text-muted-foreground" aria-hidden />}
      onClick={onClick}
    />
  );
}

export function ProvidersPage() {
  const { t } = useI18n();
  const [providers, setProviders] = useState<Provider[] | null>(null);
  const [editing, setEditing] = useState<Provider | "new" | null>(null);
  const reload = useCallback(() => {
    void adminApi.providers().then(setProviders);
  }, []);
  useEffect(reload, [reload]);

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      <VeitSettingsSection
        title={t("providersTitle")}
        description={t("providersHint")}
        actions={
          <VeitButton variant="secondary" onClick={() => setEditing("new")}>
            {t("add")}
          </VeitButton>
        }
      >
        <VeitList>
          {providers?.map((p) => <ProviderRow key={p.id} provider={p} onClick={() => setEditing(p)} />)}
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
