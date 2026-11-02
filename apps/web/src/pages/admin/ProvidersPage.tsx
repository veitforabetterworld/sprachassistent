import { useCallback, useEffect, useMemo, useState, type ComponentProps } from "react";
import { ChevronRight, GripVertical, Loader2 } from "lucide-react";
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
import { VeitSortableActivatorRow, VeitSortableVerticalList } from "@veit/react-dnd";
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

type Draft = Omit<ProviderInput, "priority">;

function toDraft(p: ProviderInput): Draft {
  const { name, base_url, model, api_key, enabled } = p;
  return { name, base_url, model, api_key, enabled };
}

function ProviderEditDialog({
  open,
  provider,
  nextPriority,
  onClose,
  onSaved,
}: {
  open: boolean;
  /** null creates a provider. */
  provider: Provider | null;
  /** Priority for a new provider, after the current last one. */
  nextPriority: number;
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

  const test = async (tools: boolean) => {
    if (!provider) return;
    setTesting(true);
    setTestResult(null);
    try {
      const r = tools ? await adminApi.testProviderTools(provider.id) : await adminApi.testProvider(provider.id);
      const called = "called" in r && r.called;
      const text = !r.ok
        ? (r.error ?? t("error"))
        : tools
          ? called
            ? tf("testToolsReply", { ms: r.ms })
            : tf("testToolsText", { reply: r.reply ?? "" })
          : tf("testReply", { reply: r.reply ?? "", ms: r.ms });
      setTestResult({ ok: r.ok, text });
    } catch (e) {
      setTestResult({ ok: false, text: errorText(e) });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    const input: ProviderInput = { ...draft, priority: provider ? provider.priority : nextPriority };
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
        <VeitSettingsToggleRow variant="embed" label={t("enabled")} checked={draft.enabled} disabled={saving} onCheckedChange={(v) => set("enabled", v)} />
        {provider && (
          <VeitField label={t("connection")} hint={dirty ? t("connectionHint") : undefined}>
            <div className="flex flex-col items-start gap-2">
              <div className="flex flex-wrap gap-2">
                <VeitButton variant="secondary" disabled={saving || testing || !ready} onClick={() => void test(false)}>
                  {testing && <Loader2 className="size-4 animate-spin" aria-hidden />}
                  {t("test")}
                </VeitButton>
                <VeitButton variant="secondary" disabled={saving || testing || !ready} onClick={() => void test(true)}>
                  {t("testTools")}
                </VeitButton>
              </div>
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

/** A provider; the grip changes the order, a click opens the dialog. */
function ProviderRow({ provider, dragDisabled, onClick }: { provider: Provider; dragDisabled: boolean; onClick: () => void }) {
  const { t, tf } = useI18n();
  const ready = provider.enabled && (provider.api_key_set || isLocal(provider.base_url));
  return (
    <VeitSortableActivatorRow
      id={provider.id}
      disabled={dragDisabled}
      draggingOpacity={0.45}
      renderActivator={({ setActivatorNodeRef, listeners }) => (
        <VeitButton
          type="button"
          variant="secondary"
          className="cursor-grab touch-manipulation active:cursor-grabbing"
          aria-label={t("dragProvider")}
          disabled={dragDisabled}
          ref={setActivatorNodeRef}
          {...(listeners as ComponentProps<typeof VeitButton>)}
        >
          <GripVertical className="size-4" aria-hidden />
        </VeitButton>
      )}
    >
      <VeitListItem
        as="div"
        title={provider.name}
        meta={tf("providerMeta", { model: provider.model })}
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
    </VeitSortableActivatorRow>
  );
}

export function ProvidersPage() {
  const { t } = useI18n();
  const [providers, setProviders] = useState<Provider[] | null>(null);
  const [editing, setEditing] = useState<Provider | "new" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const reload = useCallback(() => {
    void adminApi.providers().then(setProviders);
  }, []);
  useEffect(reload, [reload]);

  const reorder = async (activeId: string, overId: string) => {
    if (!providers || activeId === overId) return;
    const from = providers.findIndex((provider) => provider.id === activeId);
    const to = providers.findIndex((provider) => provider.id === overId);
    if (from < 0 || to < 0) return;
    const next = [...providers];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    const ranked = next.map((provider, index) => ({ ...provider, priority: (index + 1) * 10 }));
    const previous = providers;
    setProviders(ranked);
    try {
      await Promise.all(
        ranked
          .filter((provider) => previous.find((item) => item.id === provider.id)?.priority !== provider.priority)
          .map((provider) =>
            adminApi.updateProvider(provider.id, {
              name: provider.name,
              base_url: provider.base_url,
              model: provider.model,
              api_key: "",
              enabled: provider.enabled,
              priority: provider.priority,
            }),
          ),
      );
    } catch (error) {
      setProviders(previous);
      setNotice(errorText(error));
    }
  };

  const nextPriority = (providers?.reduce((max, provider) => Math.max(max, provider.priority), 0) ?? 0) + 10;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      {notice && (
        <VeitStatusBadge variant="destructive" shape="rounded">
          {notice}
        </VeitStatusBadge>
      )}
      <VeitSettingsSection
        title={t("providersTitle")}
        actions={
          <VeitButton variant="secondary" onClick={() => setEditing("new")}>
            {t("add")}
          </VeitButton>
        }
      >
        <VeitSortableVerticalList
          sortableIds={providers?.map((provider) => provider.id) ?? []}
          onDragEnd={(event) => {
            if (!event.over) return;
            void reorder(String(event.active.id), String(event.over.id));
          }}
          sortableChildren={
            <VeitList>
              {providers?.map((provider) => (
                <ProviderRow
                  key={provider.id}
                  provider={provider}
                  dragDisabled={(providers?.length ?? 0) < 2}
                  onClick={() => setEditing(provider)}
                />
              ))}
            </VeitList>
          }
        />
      </VeitSettingsSection>
      <ProviderEditDialog
        open={editing !== null}
        provider={editing === "new" ? null : editing}
        nextPriority={nextPriority}
        onClose={() => setEditing(null)}
        onSaved={() => {
          setEditing(null);
          reload();
        }}
      />
    </div>
  );
}
