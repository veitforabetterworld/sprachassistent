import { useCallback, useEffect, useState } from "react";
import { ChevronRight } from "lucide-react";
import { VeitButton, VeitEmptyState, VeitList, VeitListItem, VeitSettingsSection, VeitStatusBadge, VeitTabBar } from "@veit/react-controls";
import { adminApi, type Device, type Voice } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";
import { ChatPanel } from "./ChatPanel";
import { DeviceEditDialog } from "./DeviceEditDialog";

type Notice = { ok: boolean; text: string } | null;

function NoticeBadge({ notice }: { notice: Notice }) {
  if (!notice) return null;
  return (
    <VeitStatusBadge variant={notice.ok ? "success" : "destructive"} shape="rounded">
      {notice.text}
    </VeitStatusBadge>
  );
}

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
  const [notice, setNotice] = useState<Notice>(null);
  const [historyVersion, setHistoryVersion] = useState(0);

  const reload = useCallback(async (select?: string) => {
    const list = await adminApi.devices();
    setDevices(list);
    setSelected((s) => select ?? (s && list.some((d) => d.id === s) ? s : (list[0]?.id ?? null)));
  }, []);

  useEffect(() => {
    void reload();
    void adminApi.voices().then(setVoices).catch(() => setVoices([]));
    void adminApi.providerChoices().then(setProviders);
  }, [reload]);

  const device = devices?.find((d) => d.id === selected) ?? null;

  return (
    <div className="mx-auto flex h-full w-full max-w-3xl flex-col gap-4 p-4 sm:p-6">
      <div className="flex flex-wrap items-center gap-3">
        {devices && devices.length > 0 && (
          <VeitTabBar ariaLabel={t("devicesTabs")} value={selected ?? ""} onChange={setSelected} items={devices.map((d) => ({ id: d.id, label: d.name }))} />
        )}
        <VeitButton variant="secondary" onClick={() => setEditing("new")}>
          {t("addDevice")}
        </VeitButton>
      </div>
      <NoticeBadge notice={notice} />
      {devices?.length === 0 && <VeitEmptyState title={t("noDeviceTitle")} description={t("noDeviceText")} />}
      {device && (
        <>
          <VeitSettingsSection title={t("deviceSection")}>
            <DeviceSummary device={device} voices={voices} providers={providers} onClick={() => setEditing(device)} />
          </VeitSettingsSection>
          <ChatPanel key={device.id} deviceId={device.id} historyVersion={historyVersion} />
        </>
      )}
      <DeviceEditDialog
        open={editing !== null}
        device={editing === "new" ? null : editing}
        voices={voices}
        providers={providers}
        onClose={() => setEditing(null)}
        onSaved={(saved) => {
          setNotice(null);
          void reload(saved.id);
        }}
        onDeleted={() => {
          setEditing(null);
          void reload();
        }}
        onError={(text) => setNotice({ ok: false, text })}
        onHistoryCleared={() => setHistoryVersion((version) => version + 1)}
      />
    </div>
  );
}
