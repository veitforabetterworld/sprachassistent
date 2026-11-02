import { Link, Navigate, Outlet, useLocation } from "react-router-dom";
import { AppShell, VeitPanel, VeitSpinner } from "@veit/react-controls";
import { useAccount } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";
import { Layout, linkClass } from "../Layout";

export function AdminLayout() {
  const location = useLocation();
  const { t } = useI18n();
  const { user, loading } = useAccount();

  if (loading) {
    return (
      <AppShell variant="center">
        <VeitSpinner size="lg" label={t("loading")} />
      </AppShell>
    );
  }
  if (!user) return <Navigate to={`/anmelden?next=${encodeURIComponent(location.pathname)}`} replace />;
  if (!user.is_admin) {
    return (
      <AppShell variant="center">
        <VeitPanel as="div" className="flex w-full max-w-md flex-col gap-4">
          <p className="m-0">{t("noAdminAccess")}</p>
          <Link to="/konto" className={linkClass}>
            {t("toAccount")}
          </Link>
        </VeitPanel>
      </AppShell>
    );
  }

  return (
    <Layout>
      <Outlet />
    </Layout>
  );
}
