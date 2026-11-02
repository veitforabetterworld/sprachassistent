import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VEIT_SETTINGS_ACTIONS,
  VeitButton,
  VeitEmptyState,
  VeitField,
  VeitInput,
  VeitList,
  VeitListItem,
  VeitSecretField,
  VeitSelect,
  VeitSettingsSection,
  VeitStatusBadge,
} from "@veit/react-controls";
import { VeitOverlayDialog, VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import type { VeitApiKey } from "@veit/react-api-keys";
import { adminApi, apiKeysClient, type Device } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

const chevron = <ChevronRight className="size-4 text-muted-foreground" aria-hidden />;

type Editing = { mode: "create" } | { mode: "edit"; key: VeitApiKey };

/** One API key, one chatbot. The fixed permission stays on the server. */
export function KeysPage() {
  const { t, tf, locale, formatTime } = useI18n();
  const [devices, setDevices] = useState<Device[]>([]);
  const [keys, setKeys] = useState<VeitApiKey[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [created, setCreated] = useState<{ key: string; apiKey: VeitApiKey } | null>(null);
  const pendingCreated = useRef<{ key: string; apiKey: VeitApiKey } | null>(null);
  const errorText = useCallback(
    (err: unknown) => veitAuthErrorText(err, veitAuthT(locale, { "api_keys.one_device": t("keysOneDevice") })),
    [locale, t],
  );

  const reload = useCallback(() => {
    void Promise.all([adminApi.devices(), apiKeysClient.list()])
      .then(([list, stored]) => {
        setDevices(list);
        setKeys(stored);
        setError(null);
      })
      .catch((err: unknown) => setError(errorText(err)));
  }, [errorText]);

  useEffect(reload, [reload]);

  const chatbotName = (id: string) => devices.find((device) => device.id === id)?.name ?? t("keysMissingChatbot");

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4 sm:p-6">
      <VeitSettingsSection title={t("keysTitle")} description={t("keysDescription")}>
        <div className="flex flex-col gap-4">
          {keys === null && !error && <p className="m-0 text-sm text-muted-foreground">{t("loading")}</p>}
          {keys?.length === 0 && <VeitEmptyState embedded title={t("keysEmpty")} />}
          {keys && keys.length > 0 && (
            <VeitList>
              {keys.map((key) => {
                const chatbot = key.resources.length === 1 ? chatbotName(key.resources[0]) : t("keysUnassigned");
                const used = key.last_used_at ? tf("keysLastUsed", { time: formatTime(key.last_used_at) }) : t("keysNever");
                return (
                  <VeitListItem
                    key={key.id}
                    className={key.revoked_at ? "opacity-60" : undefined}
                    title={
                      <>
                        <span className="truncate">{key.label}</span>
                        <code className="text-xs font-normal text-muted-foreground">{key.prefix}…</code>
                      </>
                    }
                    badge={key.revoked_at ? <VeitStatusBadge variant="neutral">{t("keysRevoked")}</VeitStatusBadge> : undefined}
                    meta={`${chatbot} · ${used}`}
                    trailing={key.revoked_at ? undefined : chevron}
                    onClick={key.revoked_at ? undefined : () => setEditing({ mode: "edit", key })}
                  />
                );
              })}
            </VeitList>
          )}
          {error && <p className="m-0 text-sm text-destructive">{error}</p>}
          {keys && devices.length === 0 && <p className="m-0 text-sm text-muted-foreground">{t("keysNoChatbot")}</p>}
          <div className={VEIT_SETTINGS_ACTIONS}>
            <VeitButton disabled={keys === null || devices.length === 0} onClick={() => setEditing({ mode: "create" })}>
              {t("keysCreate")}
            </VeitButton>
          </div>
        </div>
      </VeitSettingsSection>
      <KeyDialog
        editing={editing}
        devices={devices}
        errorText={errorText}
        onClose={() => {
          setEditing(null);
          if (pendingCreated.current) setCreated(pendingCreated.current);
          pendingCreated.current = null;
        }}
        onCreated={(key, apiKey) => {
          pendingCreated.current = { key, apiKey };
          reload();
        }}
        onChanged={reload}
      />
      <CreatedKeyDialog created={created} errorText={errorText} onClose={() => setCreated(null)} />
    </div>
  );
}

function KeyDialog({
  editing,
  devices,
  errorText,
  onClose,
  onCreated,
  onChanged,
}: {
  editing: Editing | null;
  devices: Device[];
  errorText: (error: unknown) => string;
  onClose: () => void;
  onCreated: (key: string, apiKey: VeitApiKey) => void;
  onChanged: () => void;
}) {
  const { t } = useI18n();
  const open = editing !== null;
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const current = editing?.mode === "edit" ? editing.key : null;
  const initial = useMemo(
    () => ({
      label: current?.label ?? "",
      chatbot: current?.resources.length === 1 ? current.resources[0] : (devices[0]?.id ?? ""),
    }),
    [current, devices],
  );
  const [draft, setDraft] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { dirty, formBaseline } = useDialogFormBaseline({ draft, resolveBaseline: () => initial, resetDeps: [open, initial], track: open });

  useEffect(() => {
    if (open) {
      setDraft(initial);
      setError(null);
    }
  }, [open, initial]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorText(err));
      throw err;
    } finally {
      setBusy(false);
    }
  };

  const body = {
    label: draft.label.trim(),
    permissions: ["device.talk"],
    all_resources: false,
    resources: [draft.chatbot],
  };

  return (
    <VeitOverlayEditDialog
      binding={binding}
      title={current ? current.label : t("keysCreate")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="lg"
      disabled={busy}
      blockBackdropClose={busy}
      footerProps={{
        busy,
        dirty,
        formBaseline,
        saveRequiresDirty: current !== null,
        saveDisabled: !draft.label.trim() || !draft.chatbot,
        cancelLabel: t("cancel"),
        saveLabel: current ? t("save") : t("keysCreate"),
        onSave: () =>
          run(async () => {
            if (current) {
              await apiKeysClient.update(current.id, body);
              onChanged();
            } else {
              const created = await apiKeysClient.create(body);
              onCreated(created.key, created.apiKey);
            }
          }),
        deleteAction: current
          ? {
              ariaLabel: t("keysRevoke"),
              confirm: {
                title: t("keysRevokeTitle"),
                message: t("keysRevokeConfirm"),
                confirmLabel: t("keysRevoke"),
                cancelLabel: t("cancel"),
                destructive: true,
              },
              onDelete: () =>
                run(async () => {
                  await apiKeysClient.revoke(current.id);
                  onChanged();
                }),
            }
          : undefined,
      }}
    >
      <div className="flex flex-col gap-4">
        {current && <KeyField apiKey={current} errorText={errorText} />}
        <VeitField label={t("name")} htmlFor="api-key-label">
          <VeitInput
            id="api-key-label"
            value={draft.label}
            maxLength={120}
            disabled={busy}
            onChange={(e) => setDraft((d) => ({ ...d, label: e.target.value }))}
          />
        </VeitField>
        <VeitField label={t("keysChatbot")} htmlFor="api-key-chatbot">
          <VeitSelect
            id="api-key-chatbot"
            value={draft.chatbot}
            disabled={busy}
            onValueChange={(chatbot) => setDraft((d) => ({ ...d, chatbot }))}
            options={devices.map((device) => ({ value: device.id, label: device.name }))}
          />
        </VeitField>
        {error && <p className="m-0 text-sm text-destructive">{error}</p>}
      </div>
    </VeitOverlayEditDialog>
  );
}

function KeyField({ apiKey, known, errorText }: { apiKey: VeitApiKey; known?: string; errorText: (error: unknown) => string }) {
  const { t } = useI18n();
  const [error, setError] = useState<string | null>(null);
  const revealable = Boolean(apiKey.revealable) && Boolean(apiKeysClient.reveal);
  if (!known && !revealable) {
    return (
      <VeitField label={t("keysSecret")} htmlFor={`api-key-${apiKey.id}`}>
        <VeitInput id={`api-key-${apiKey.id}`} readOnly className="font-mono" value={`${apiKey.prefix}…`} />
      </VeitField>
    );
  }
  return (
    <VeitField label={t("keysSecret")} htmlFor={`api-key-${apiKey.id}`} error={error ?? undefined}>
      <VeitSecretField
        id={`api-key-${apiKey.id}`}
        value={known ?? ""}
        stored={!known}
        onReveal={() => apiKeysClient.reveal!(apiKey.id)}
        onRevealError={(err) => setError(errorText(err))}
        showLabel={t("showKey")}
        hideLabel={t("hideKey")}
        copyLabel={t("copy")}
        copiedLabel={t("copied")}
        autoComplete="off"
      />
    </VeitField>
  );
}

function CreatedKeyDialog({
  created,
  errorText,
  onClose,
}: {
  created: { key: string; apiKey: VeitApiKey } | null;
  errorText: (error: unknown) => string;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const binding = useManagedOverlayDialogBinding(created !== null, onClose);
  return (
    <VeitOverlayDialog binding={binding} title={t("keysCreated")} closeAriaLabel={t("close")} variant="responsive" size="lg">
      {created && (
        <div className="flex flex-col gap-4">
          <p className="m-0 text-sm text-muted-foreground">{t("keysCreatedHint")}</p>
          <KeyField apiKey={created.apiKey} known={created.key} errorText={errorText} />
          <div className={VEIT_SETTINGS_ACTIONS}>
            <VeitButton onClick={onClose}>{t("close")}</VeitButton>
          </div>
        </div>
      )}
    </VeitOverlayDialog>
  );
}
