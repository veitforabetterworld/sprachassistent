import { useEffect, useMemo, useState } from "react";
import { VeitApiKeysSection, veitApiKeysTexts, type VeitApiKeysTexts } from "@veit/react-api-keys";
import { VeitAlert, VeitPage } from "@veit/react-controls";
import { DEVICE_TALK, adminApi, apiKeysClient, errorText, type Device } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

/** One API key per chatbot with the fixed permission device.talk (checked again on the server). */
export function KeysPage() {
  const { t, locale, formatTime } = useI18n();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    adminApi.devices().then(setDevices, (err: unknown) => setError(errorText(err)));
  }, []);

  const texts = useMemo<VeitApiKeysTexts>(
    () => ({
      ...veitApiKeysTexts[locale],
      title: t("keysTitle"),
      description: t("keysDescription"),
      empty: t("keysEmpty"),
      create: t("keysCreate"),
      revokeTitle: t("keysRevokeTitle"),
      revokeConfirm: t("keysRevokeConfirm"),
      newKey: t("keysCreated"),
      newKeyHint: t("keysCreatedHint"),
      noResources: t("keysNoChatbot"),
      key: t("keysSecret"),
      resource: t("keysChatbot"),
      unassigned: t("keysUnassigned"),
      loading: t("loading"),
    }),
    [locale, t],
  );
  const resources = useMemo(() => (devices ?? []).map((device) => ({ id: device.id, label: device.name })), [devices]);

  return (
    <VeitPage width="narrow">
      {error && <VeitAlert>{error}</VeitAlert>}
      {devices && (
        <VeitApiKeysSection
          client={apiKeysClient}
          permissions={[]}
          fixedPermissions={[DEVICE_TALK]}
          resourceMode="single"
          resources={resources}
          resourceNoun={t("keysChatbot")}
          texts={texts}
          errorText={errorText}
          errorCodeText={(code) => (code === "api_keys.one_device" ? t("keysOneDevice") : undefined)}
          formatTime={formatTime}
        />
      )}
    </VeitPage>
  );
}
