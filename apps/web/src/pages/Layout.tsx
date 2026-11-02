import { useEffect, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { AppShell, VeitAppHeader, VeitButton, VeitStatusBadge } from "@veit/react-controls";
import { adminApi, useAccount, type Health } from "../api";
import type { MessageKey } from "../i18n/de";
import { useI18n } from "../i18n/I18nProvider";

/** Inline text links, as in Hochwasserdienst. */
export const linkClass = "text-primary no-underline hover:underline";

const ADMIN_NAV: Array<{ to: string; label: MessageKey }> = [
  { to: "/admin/geraete", label: "navDevices" },
  { to: "/admin/schluessel", label: "navKeys" },
  { to: "/admin/ki-anbieter", label: "navProviders" },
  { to: "/admin/einstellungen", label: "navSettings" },
];

function navClass(active: boolean): string {
  return `whitespace-nowrap rounded-full px-3 py-1.5 text-sm font-medium no-underline ${
    active ? "bg-primary/10 text-primary" : "text-muted-foreground hover:bg-muted hover:text-foreground"
  }`;
}

/** Status of the voice server in the header. */
function VoiceStatus() {
  const { t, tf } = useI18n();
  const [health, setHealth] = useState<Health | null | "error">(null);
  useEffect(() => {
    adminApi.status().then(setHealth, () => setHealth("error"));
  }, []);
  if (health === null) return null;
  if (health === "error") return <VeitStatusBadge variant="destructive">{t("voiceServerDown")}</VeitStatusBadge>;
  const ok = health.db && health.stt && health.voices.length > 0 && health.providers.length > 0;
  return (
    <VeitStatusBadge variant={ok ? "success" : "warning"}>
      {health.providers.length === 0 ? t("noProviderWithKey") : tf("activeProvider", { name: health.providers[0] })}
    </VeitStatusBadge>
  );
}

/** The other locale, as a short label for the switch. */
export function LocaleSwitch() {
  const { t, locale, setLocale } = useI18n();
  return (
    <VeitButton variant="ghost" aria-label={t("switchLocale")} title={t("switchLocale")} onClick={() => setLocale(locale === "de" ? "en" : "de")}>
      {locale === "de" ? "EN" : "DE"}
    </VeitButton>
  );
}

/** Header with the admin tabs and the account; the same on every signed-in page. */
export function Layout({ children }: { children: ReactNode }) {
  const location = useLocation();
  const navigate = useNavigate();
  const { t } = useI18n();
  const { user, logout } = useAccount();
  const nav = [...(user?.is_admin ? ADMIN_NAV : []), { to: "/konto", label: "navAccount" as MessageKey }];

  return (
    <AppShell variant="viewport">
      <VeitAppHeader
        className="shrink-0"
        brand={
          <Link to={user?.is_admin ? "/admin" : "/konto"} className="text-inherit no-underline">
            <strong className="text-base">{t("appName")}</strong>
          </Link>
        }
        nav={
          <nav className="flex min-w-0 flex-wrap items-center gap-1" aria-label={t("mainNavigation")}>
            {nav.map((item) => (
              <Link key={item.to} to={item.to} className={navClass(location.pathname.startsWith(item.to))}>
                {t(item.label)}
              </Link>
            ))}
          </nav>
        }
        tools={
          <>
            {user?.is_admin && <VoiceStatus />}
            <LocaleSwitch />
            <VeitButton variant="secondary" onClick={() => void logout().then(() => navigate("/anmelden", { replace: true }))}>
              {t("logout")}
            </VeitButton>
          </>
        }
      />
      <main className="min-h-0 flex-1 overflow-auto">{children}</main>
    </AppShell>
  );
}
