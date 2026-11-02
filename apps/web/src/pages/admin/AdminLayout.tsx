import { Navigate, Outlet, useLocation } from "react-router-dom";
import { AppShell, VeitSpinner } from "@veit/react-controls";
import { useAccount } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";
import { Layout } from "../Layout";

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

  return (
    <Layout>
      <Outlet />
    </Layout>
  );
}
