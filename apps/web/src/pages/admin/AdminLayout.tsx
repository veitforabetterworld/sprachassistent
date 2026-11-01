import { useEffect, useState } from "react";
import { Link, Navigate, Outlet, useLocation, useNavigate } from "react-router-dom";
import { AppShell, VeitAppHeader, VeitButton, VeitPanel, VeitSpinner, VeitStatusBadge, VeitTabBar } from "@veit/react-controls";
import { adminApi, useAccount, type Health } from "../../api";

const TABS = [
  { id: "geraete", label: "Geräte" },
  { id: "schluessel", label: "ESP-Schlüssel" },
  { id: "ki-anbieter", label: "KI-Anbieter" },
  { id: "einstellungen", label: "Einstellungen" },
];

/** Status of the voice server in the header. */
function VoiceStatus() {
  const [health, setHealth] = useState<Health | null | "error">(null);
  useEffect(() => {
    adminApi.status().then(setHealth, () => setHealth("error"));
  }, []);
  if (health === null) return null;
  if (health === "error") return <VeitStatusBadge variant="destructive">Sprachserver aus</VeitStatusBadge>;
  const ok = health.db && health.stt && health.voices.length > 0 && health.providers.length > 0;
  return (
    <VeitStatusBadge variant={ok ? "success" : "warning"}>
      {health.providers.length === 0 ? "kein KI-Anbieter mit Key" : `KI: ${health.providers[0]}`}
    </VeitStatusBadge>
  );
}

export function AdminLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { user, loading, logout } = useAccount();
  const tab = TABS.find((t) => location.pathname.includes(t.id))?.id ?? "geraete";

  if (loading) {
    return (
      <AppShell variant="center">
        <VeitSpinner size="lg" label="Lädt" />
      </AppShell>
    );
  }
  if (!user) return <Navigate to={`/anmelden?next=${encodeURIComponent(location.pathname)}`} replace />;
  if (!user.is_admin) {
    return (
      <AppShell variant="center">
        <VeitPanel as="div" className="flex w-full max-w-md flex-col gap-4">
          <p className="m-0">Dieses Konto hat keinen Zugang zum Adminbereich.</p>
          <Link to="/konto">Zum Konto</Link>
        </VeitPanel>
      </AppShell>
    );
  }

  return (
    <AppShell variant="viewport">
      <VeitAppHeader
        className="shrink-0"
        brand={<strong>Sprachassistent</strong>}
        nav={<VeitTabBar ariaLabel="Adminbereich" value={tab} onChange={(id) => navigate(`/admin/${id}`)} items={TABS} />}
        tools={
          <>
            <VoiceStatus />
            <Link className="btn-secondary btn-sm no-underline" to="/konto">
              Konto
            </Link>
            <VeitButton
              type="button"
              variant="secondary"
              className="btn-sm"
              onClick={() => void logout().then(() => navigate("/anmelden", { replace: true }))}
            >
              Abmelden
            </VeitButton>
          </>
        }
      />
      <main className="min-h-0 flex-1 overflow-auto">
        <Outlet />
      </main>
    </AppShell>
  );
}
