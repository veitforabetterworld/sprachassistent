import { useCallback, useEffect, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VEIT_SETTINGS_ACTIONS,
  VeitAlert,
  VeitButton,
  VeitEmptyState,
  VeitList,
  VeitListItem,
  VeitPage,
  VeitSettingsSection,
  VeitSpinner,
  VeitTabBar,
} from "@veit/react-controls";
import { adminApi, errorText, type Device, type Voice } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";
import { ChatPanel } from "./ChatPanel";
import { DeviceEditDialog } from "./DeviceEditDialog";

const chevron = <ChevronRight className="size-4 text-muted-foreground" aria-hidden />;

/** Overview of a chatbot; a click opens the settings. */
function DeviceSummary({ device, voices, providers, onClick }: { device: Device; voices: Voice[]; providers: { id: string; name: string }[]; onClick: () => void }) {
  const { t, tf } = useI18n();
  const voice = voices.find((v) => v.id === device.voice)?.label ?? device.voice;
  const provider = providers.find((p) => p.id === device.provider_id)?.name ?? t("defaultProvider");
  return (
    <VeitList>
      <VeitListItem
        title={device.name}
        meta={
          <div className="flex flex-col gap-1">
            <span>{tf("deviceSummary", { voice, speed: device.speed, provider })}</span>
            <span className="line-clamp-2">{device.persona || t("noPersona")}</span>
          </div>
        }
        trailing={chevron}
        onClick={onClick}
      />
    </VeitList>
  );
}

export function DevicesPage() {
  const { t } = useI18n();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [voices, setVoices] = useState<Voice[]>([]);
  const [providers, setProviders] = useState<{ id: string; name: string }[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<Device | "new" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [historyVersion, setHistoryVersion] = useState(0);

  const reload = useCallback(async (select?: string) => {
    const list = await adminApi.devices();
    setDevices(list);
    setSelected((s) => select ?? (s && list.some((d) => d.id === s) ? s : (list[0]?.id ?? null)));
  }, []);

  useEffect(() => {
    reload().catch((e: unknown) => setError(errorText(e)));
    void adminApi.voices().then(setVoices).catch(() => setVoices([]));
    void adminApi.providerChoices().then(setProviders);
  }, [reload]);

  const device = devices?.find((d) => d.id === selected) ?? null;

  return (
    <VeitPage width="narrow" className="h-full">
      {error && <VeitAlert>{error}</VeitAlert>}
      {devices === null && !error && <VeitSpinner label={t("loading")} />}
      {devices && devices.length > 0 && (
        <VeitTabBar ariaLabel={t("devicesTabs")} value={selected ?? ""} onChange={setSelected} items={devices.map((d) => ({ id: d.id, label: d.name }))} />
      )}
      {devices && (
        <VeitSettingsSection title={t("deviceSection")}>
          <div className="flex flex-col gap-4">
            {device ? (
              <DeviceSummary device={device} voices={voices} providers={providers} onClick={() => setEditing(device)} />
            ) : (
              <VeitEmptyState embedded title={t("noDeviceTitle")} description={t("noDeviceText")} />
            )}
            <div className={VEIT_SETTINGS_ACTIONS}>
              <VeitButton onClick={() => setEditing("new")}>{t("addDevice")}</VeitButton>
            </div>
          </div>
        </VeitSettingsSection>
      )}
      {device && (
        <>
          <ChatPanel key={device.id} deviceId={device.id} historyVersion={historyVersion} />
        </>
      )}
      <DeviceEditDialog
        open={editing !== null}
        device={editing === "new" ? null : editing}
        voices={voices}
        providers={providers}
        onClose={() => setEditing(null)}
        onSaved={(saved) => void reload(saved.id)}
        onDeleted={() => {
          setEditing(null);
          void reload();
        }}
        onHistoryCleared={() => setHistoryVersion((version) => version + 1)}
      />
    </VeitPage>
  );
}
