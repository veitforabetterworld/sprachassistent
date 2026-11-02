import { useMemo, type ReactNode } from "react";
import { Navigate, Route, Routes } from "react-router-dom";
import { createVeitDialogUnsavedConfirm, VeitDialogUnsavedConfirmProvider } from "@veit/react-dialog";
import { AuthProvider } from "./api";
import { I18nProvider, useI18n } from "./i18n/I18nProvider";
import { AccountPage } from "./pages/AccountPage";
import { ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, VerifyEmailPage } from "./pages/AuthPages";
import { AdminLayout } from "./pages/admin/AdminLayout";
import { DevicesPage } from "./pages/admin/DevicesPage";
import { KeysPage } from "./pages/admin/KeysPage";
import { ProvidersPage } from "./pages/admin/ProvidersPage";
import { SettingsPage } from "./pages/admin/SettingsPage";

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

export function App() {
  return (
    <I18nProvider>
      <AuthProvider>
        <UnsavedConfirm>
          <Routes>
            <Route path="/admin" element={<AdminLayout />}>
              <Route index element={<Navigate to="geraete" replace />} />
              <Route path="geraete" element={<DevicesPage />} />
              <Route path="schluessel" element={<KeysPage />} />
              <Route path="ki-anbieter" element={<ProvidersPage />} />
              <Route path="einstellungen" element={<SettingsPage />} />
            </Route>
            <Route path="/konto" element={<AccountPage />} />
            <Route path="/anmelden" element={<LoginPage />} />
            <Route path="/registrieren" element={<RegisterPage />} />
            <Route path="/passwort-vergessen" element={<ForgotPasswordPage />} />
            <Route path="/passwort-neu" element={<ResetPasswordPage />} />
            <Route path="/e-mail-bestaetigen" element={<VerifyEmailPage />} />
            <Route path="*" element={<Navigate to="/admin" replace />} />
          </Routes>
        </UnsavedConfirm>
      </AuthProvider>
    </I18nProvider>
  );
}
