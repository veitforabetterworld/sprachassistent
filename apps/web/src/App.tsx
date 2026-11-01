import { Navigate, Route, Routes } from "react-router-dom";
import { createVeitDialogUnsavedConfirm, VeitDialogUnsavedConfirmProvider } from "@veit/react-dialog";
import { AuthProvider } from "./api";
import { AccountPage } from "./pages/AccountPage";
import { ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, VerifyEmailPage } from "./pages/AuthPages";
import { AdminLayout } from "./pages/admin/AdminLayout";
import { DevicesPage } from "./pages/admin/DevicesPage";
import { KeysPage } from "./pages/admin/KeysPage";
import { ProvidersPage } from "./pages/admin/ProvidersPage";
import { SettingsPage } from "./pages/admin/SettingsPage";

/** Asked when an edit dialog with unsaved changes is closed. */
const unsavedConfirm = createVeitDialogUnsavedConfirm({
  title: "Ungespeicherte Änderungen",
  message: "Die Änderungen gehen verloren, wenn du den Dialog jetzt schließt.",
  saveLabel: "Speichern",
  confirmLabel: "Verwerfen",
  cancelLabel: "Weiter bearbeiten",
  closeAriaLabel: "Schließen",
});

export function App() {
  return (
    <AuthProvider>
      <VeitDialogUnsavedConfirmProvider value={unsavedConfirm}>
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
      </VeitDialogUnsavedConfirmProvider>
    </AuthProvider>
  );
}
