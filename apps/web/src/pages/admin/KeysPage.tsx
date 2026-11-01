import { useEffect, useState } from "react";
import { VeitApiKeysSection, veitApiKeysTexts } from "@veit/react-api-keys";
import { veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import { adminApi, apiKeysClient, type Device } from "../../api";

const authT = veitAuthT("de");

/** ESP keys: framework API keys with the permission device.talk, limited to devices. */
export function KeysPage() {
  const [devices, setDevices] = useState<Device[]>([]);
  useEffect(() => {
    void adminApi.devices().then(setDevices);
  }, []);

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4 sm:p-6">
      <p className="m-0 text-sm text-muted-foreground">
        Jeder ESP bekommt einen eigenen Schlüssel, beschränkt auf sein Gerät. Der Schlüssel wird nur einmal angezeigt
        und kommt in <code>firmware/include/secrets.h</code> als <code>API_KEY</code>. Ein Schlüssel für mehrere Geräte
        braucht beim Aufruf <code>?device=&lt;id&gt;</code>.
      </p>
      <VeitApiKeysSection
        client={apiKeysClient}
        permissions={[{ id: "device.talk", label: "Sprechen", description: "Darf mit dem Gerät sprechen (/v1/talk)" }]}
        resources={devices.map((d) => ({ id: d.id, label: d.name }))}
        resourceNoun="Geräte"
        texts={veitApiKeysTexts.de}
        errorText={(error: unknown) => veitAuthErrorText(error, authT)}
        formatTime={(iso: string) => new Date(iso).toLocaleString("de-DE", { dateStyle: "short", timeStyle: "short" })}
      />
    </div>
  );
}
