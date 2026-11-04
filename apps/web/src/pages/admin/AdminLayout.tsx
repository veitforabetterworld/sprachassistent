import type { ReactNode } from "react";
import { Outlet } from "react-router-dom";
import { ProtectedRoute } from "@veit/react-auth";
import { AppShell, VeitSpinner } from "@veit/react-controls";
import { useI18n } from "../../i18n/I18nProvider";
import { Layout } from "../Layout";

/** Signed-in pages; without a session to the login, which returns here afterwards (router state). */
export function SignedIn({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  return (
    <ProtectedRoute
      loginPath="/anmelden"
      loadingFallback={
        <AppShell variant="center">
          <VeitSpinner size="lg" label={t("loading")} />
        </AppShell>
      }
    >
      <Layout>{children}</Layout>
    </ProtectedRoute>
  );
}

export function AdminLayout() {
  return (
    <SignedIn>
      <Outlet />
    </SignedIn>
  );
}
