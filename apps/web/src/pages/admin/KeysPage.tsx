import { useEffect, useState } from "react";
import { VeitApiKeysSection, veitApiKeysTexts } from "@veit/react-api-keys";
import { veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import { adminApi, apiKeysClient, type Device } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

/** ESP keys: framework API keys with the permission device.talk, limited to devices. */
export function KeysPage() {
  const { t, locale, formatTime } = useI18n();
  const [devices, setDevices] = useState<Device[]>([]);
  useEffect(() => {
    void adminApi.devices().then(setDevices);
  }, []);
  const authT = veitAuthT(locale);

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4 sm:p-6">
      <p className="m-0 text-sm text-muted-foreground">{t("keysIntro")}</p>
      <VeitApiKeysSection
        client={apiKeysClient}
        permissions={[{ id: "device.talk", label: t("permissionTalk"), description: t("permissionTalkHint") }]}
        resources={devices.map((d) => ({ id: d.id, label: d.name }))}
        resourceNoun={t("devicesNoun")}
        texts={veitApiKeysTexts[locale]}
        errorText={(error: unknown) => veitAuthErrorText(error, authT)}
        formatTime={formatTime}
      />
    </div>
  );
}
