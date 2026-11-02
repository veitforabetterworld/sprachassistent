import { useState, useMemo, type ReactNode } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router-dom";
import { AppShell, VeitButton } from "@veit/react-controls";
import { createVeitDialogUnsavedConfirm, VeitDialogUnsavedConfirmProvider } from "@veit/react-dialog";
import { AuthProvider, authClient, errorText, useAccount } from "./api";
import { I18nProvider, useI18n } from "./i18n/I18nProvider";
import { AccountPage } from "./pages/AccountPage";
import { ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, VerifyEmailPage } from "./pages/AuthPages";
import { AdminLayout } from "./pages/admin/AdminLayout";
import { DevicesPage } from "./pages/admin/DevicesPage";
import { KeysPage } from "./pages/admin/KeysPage";
import { McpPage } from "./pages/admin/McpPage";
import { ProvidersPage } from "./pages/admin/ProvidersPage";
import { SettingsPage } from "./pages/admin/SettingsPage";
import { UsersPage } from "./pages/admin/UsersPage";

/** Asked when an edit dialog with unsaved changes is closed. */
function UnsavedConfirm({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  const value = useMemo(
    () =>
      createVeitDialogUnsavedConfirm({
        title: t("unsavedTitle"),
        message: t("unsavedMessage"),
        saveLabel: t("save"),
        confirmLabel: t("unsavedDiscard"),
        cancelLabel: t("unsavedKeepEditing"),
        closeAriaLabel: t("close"),
      }),
    [t],
  );
  return <VeitDialogUnsavedConfirmProvider value={value}>{children}</VeitDialogUnsavedConfirmProvider>;
}

const OPEN_PATHS = new Set(["/anmelden", "/registrieren", "/passwort-vergessen", "/passwort-neu", "/e-mail-bestaetigen"]);

function VerifyGate() {
  const { t } = useI18n();
  const { logout, refreshUser } = useAccount();
  const [notice, setNotice] = useState<string | null>(null);
  return (
    <AppShell variant="center">
      <div className="flex max-w-md flex-col gap-4 p-6">
        <h1 className="m-0 text-xl font-semibold">{t("verifyGateTitle")}</h1>
        <p className="m-0 text-sm text-muted-foreground">{t("verifyGateText")}</p>
        {notice && <p className="m-0 text-sm text-muted-foreground">{notice}</p>}
        <VeitButton
          onClick={() =>
            void authClient.resendVerification().then(
              () => setNotice(t("verifyGateSent")),
              (error: unknown) => setNotice(errorText(error)),
            )
          }
        >
          {t("verifyGateResend")}
        </VeitButton>
        <VeitButton variant="secondary" onClick={() => void refreshUser()}>
          {t("verifyGateCheck")}
        </VeitButton>
        <VeitButton variant="secondary" onClick={() => void logout()}>
          {t("logout")}
        </VeitButton>
      </div>
    </AppShell>
  );
}

function AccountGate({ children }: { children: ReactNode }) {
  const location = useLocation();
  const { user, loading } = useAccount();
  if (loading || !user || OPEN_PATHS.has(location.pathname)) return children;
  if (user.verification_required && user.email_verified_at == null && !user.is_admin) return <VerifyGate />;
  return children;
}

function AdminOnly({ children }: { children: ReactNode }) {
  const { user, loading } = useAccount();
  if (loading) return null;
  if (!user?.is_admin) return <Navigate to="/chatbots" replace />;
  return children;
}

export function App() {
  return (
    <I18nProvider>
      <AuthProvider>
        <UnsavedConfirm>
          <AccountGate>
          <Routes>
            <Route element={<AdminLayout />}>
              <Route path="/chatbots" element={<DevicesPage />} />
              <Route path="/api-schluessel" element={<KeysPage />} />
              <Route path="/werkzeuge" element={<McpPage />} />
              <Route path="/ki-anbieter" element={<ProvidersPage />} />
              <Route path="/admin/benutzer" element={<AdminOnly><UsersPage /></AdminOnly>} />
              <Route path="/admin/einstellungen" element={<AdminOnly><SettingsPage /></AdminOnly>} />
            </Route>
            <Route path="/admin" element={<Navigate to="/chatbots" replace />} />
            <Route path="/admin/geraete" element={<Navigate to="/chatbots" replace />} />
            <Route path="/admin/schluessel" element={<Navigate to="/api-schluessel" replace />} />
            <Route path="/admin/mcp" element={<Navigate to="/werkzeuge" replace />} />
            <Route path="/admin/ki-anbieter" element={<Navigate to="/ki-anbieter" replace />} />
            <Route path="/konto" element={<AccountPage />} />
            <Route path="/anmelden" element={<LoginPage />} />
            <Route path="/registrieren" element={<RegisterPage />} />
            <Route path="/passwort-vergessen" element={<ForgotPasswordPage />} />
            <Route path="/passwort-neu" element={<ResetPasswordPage />} />
            <Route path="/e-mail-bestaetigen" element={<VerifyEmailPage />} />
            <Route path="*" element={<Navigate to="/chatbots" replace />} />
          </Routes>
          </AccountGate>
        </UnsavedConfirm>
      </AuthProvider>
    </I18nProvider>
  );
}
