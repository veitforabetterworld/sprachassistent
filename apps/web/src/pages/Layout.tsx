import type { ReactNode } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { AppShell, VeitAppHeader, VeitButton } from "@veit/react-controls";
import { useAccount } from "../api";
import type { MessageKey } from "../i18n/de";
import { useI18n } from "../i18n/I18nProvider";

/** Inline text links, as in Hochwasserdienst. */
export const linkClass = "text-primary no-underline hover:underline";

const USER_NAV: Array<{ to: string; label: MessageKey }> = [
  { to: "/chatbots", label: "navDevices" },
  { to: "/api-schluessel", label: "navKeys" },
  { to: "/werkzeuge", label: "navMcp" },
  { to: "/ki-anbieter", label: "navProviders" },
];
const ADMIN_NAV: Array<{ to: string; label: MessageKey }> = [
  { to: "/admin/einstellungen", label: "navSettings" },
];

function navClass(active: boolean): string {
  return `whitespace-nowrap rounded-full px-3 py-1.5 text-sm font-medium no-underline ${
    active ? "bg-primary/10 text-primary" : "text-muted-foreground hover:bg-muted hover:text-foreground"
  }`;
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
  const nav = [...(user ? USER_NAV : []), ...(user?.is_admin ? ADMIN_NAV : []), { to: "/konto", label: "navAccount" as MessageKey }];

  return (
    <AppShell variant="viewport">
      <VeitAppHeader
        className="shrink-0"
        brand={
          <Link to={user ? "/chatbots" : "/konto"} className="text-inherit no-underline">
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
