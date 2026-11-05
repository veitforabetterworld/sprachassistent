import { useCallback, useEffect, useMemo, useState } from "react";
import {
  VeitSettingsGroups,
  veitSettingText,
  veitSettingsGroupsTexts,
  veitSmtpSettingsGroup,
  veitSmtpSettingsTexts,
  type VeitSettingsGroup,
  type VeitSettingsValues,
} from "@veit/react-app-settings";
import { VeitAlert, VeitPage } from "@veit/react-controls";
import { adminApi, codeText, errorDetail, errorText, settingsClient } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

/** The test mail's error with what the mail server said, so a wrong password or port shows up. */
function testMailErrorText(error: unknown): string {
  const detail = errorDetail(error);
  return detail ? `${errorText(error)} (${detail})` : errorText(error);
}

/** Admin settings (@veit/app-settings): e-mail, Google sign-in for tools, speech. */
export function SettingsPage() {
  const { t, tf, locale } = useI18n();
  const [values, setValues] = useState<VeitSettingsValues | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(() => {
    settingsClient.list().then(setValues, (e: unknown) => setError(errorText(e)));
  }, []);
  useEffect(reload, [reload]);

  const groups = useMemo<VeitSettingsGroup[]>(
    () => [
      veitSmtpSettingsGroup(veitSmtpSettingsTexts[locale], { onTestMail: adminApi.testMail, errorText: testMailErrorText }),
      {
        id: "tools",
        title: t("toolsTitle"),
        description: t("toolsHint"),
        fields: [
          { key: "google_oauth_client_id", label: t("googleClientId"), kind: "text", hint: t("googleOAuthHint") },
          { key: "google_oauth_client_secret", label: t("googleClientSecret"), kind: "secret" },
        ],
        summary: (v) => tf("toolsSummary", { google: veitSettingText(v, "google_oauth_client_id") ? t("on") : t("off") }),
      },
      {
        id: "speech",
        title: t("speechTitle"),
        description: t("speechHint"),
        fields: [
          { key: "stt_model", label: t("sttModel"), kind: "text", hint: t("sttModelHint") },
          { key: "stt_threads", label: t("sttThreads"), kind: "number", min: 1, max: 8, hint: t("sttThreadsHint") },
          { key: "tts_threads", label: t("ttsThreads"), kind: "number", min: 1, max: 8 },
        ],
        summary: (v) =>
          tf("speechSummary", {
            model: veitSettingText(v, "stt_model").split("/").pop() ?? "",
            threads: veitSettingText(v, "stt_threads"),
          }),
      },
    ],
    [locale, t, tf],
  );

  return (
    <VeitPage width="narrow">
      {error && <VeitAlert>{error}</VeitAlert>}
      {/* null while loading: the list shows a spinner. Field errors of the server stand at the field. */}
      {!(error && values === null) && (
        <VeitSettingsGroups
        title={t("settingsTitle")}
        groups={groups}
        values={values}
        texts={veitSettingsGroupsTexts[locale]}
        errorText={errorText}
        fieldErrorText={codeText}
        onSave={async (patch) => setValues(await settingsClient.save(patch))}
        onReveal={settingsClient.reveal}
        />
      )}
    </VeitPage>
  );
}
