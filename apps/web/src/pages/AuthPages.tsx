import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ForgotPasswordForm, LoginForm, RegisterForm, ResetPasswordForm, VerifyEmailView, veitAuthErrorText, veitAuthT, veitFetchJson } from "@veit/react-auth";
import { VeitSettingsToggleRow } from "@veit/react-controls";
import { authClient, useAccount, type AccountUser } from "../api";

/** Pages around the account forms of @veit/react-auth; texts come from the framework. */

const t = veitAuthT("de");
const parseError = (error: unknown) => veitAuthErrorText(error, t);
const AFTER_LOGIN = "/admin";

function Brand() {
  return <strong className="text-xl">Sprachassistent</strong>;
}

function FooterLink({ to, label }: { to: string; label: string }) {
  return (
    <p className="mt-4 text-center text-sm">
      <Link to={to}>{label}</Link>
    </p>
  );
}

/** Registration is only open for the first account (the admin) unless the admin area opens it. */
function useRegistrationOpen(): boolean {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    veitFetchJson<{ admin_exists: boolean; registration_open: boolean }>("/v1/setup")
      .then((s) => setOpen(!s.admin_exists || s.registration_open))
      .catch(() => setOpen(false));
  }, []);
  return open;
}

export function LoginPage() {
  const registrationOpen = useRegistrationOpen();
  return (
    <LoginForm<AccountUser>
      t={t}
      parseError={parseError}
      afterLoginPath={AFTER_LOGIN}
      onSendMfaEmailCode={async (token) => {
        const result = await authClient.sendLoginMfaEmailCode!(token);
        return t(result.messageKey ?? "auth.mfa_email_sent");
      }}
      renderLogo={() => <Brand />}
      renderFooter={() => (
        <>
          <FooterLink to="/passwort-vergessen" label={t("auth.forgot_password")} />
          {registrationOpen && <FooterLink to="/registrieren" label="Konto anlegen (erstes Konto wird Admin)" />}
        </>
      )}
    />
  );
}

export function RegisterPage() {
  return (
    <RegisterForm<AccountUser>
      t={t}
      parseError={parseError}
      afterRegisterPath={AFTER_LOGIN}
      onSuggestPassword={() => authClient.suggestPassword()}
      renderLogo={() => <Brand />}
      renderPrivacyConsent={(checked, onCheckedChange) => (
        <VeitSettingsToggleRow
          variant="embed"
          label="Ich bin einverstanden, dass Name und E-Mail für das Konto gespeichert werden."
          checked={checked}
          onCheckedChange={onCheckedChange}
        />
      )}
      renderFooter={() => <FooterLink to="/anmelden" label="Schon ein Konto? Anmelden" />}
    />
  );
}

export function ForgotPasswordPage() {
  return (
    <ForgotPasswordForm<AccountUser>
      t={t}
      parseError={parseError}
      onRequestReset={(email) => authClient.requestPasswordReset!(email)}
      renderLogo={() => <Brand />}
      renderFooter={() => <FooterLink to="/anmelden" label="Zurück zur Anmeldung" />}
    />
  );
}

export function ResetPasswordPage() {
  return (
    <ResetPasswordForm
      t={t}
      parseError={parseError}
      onConfirmReset={(token, password) => authClient.confirmPasswordReset!(token, password)}
      renderLogo={() => <Brand />}
    />
  );
}

export function VerifyEmailPage() {
  const { user, refreshUser } = useAccount();
  return (
    <VerifyEmailView
      t={t}
      onVerify={async (token) => {
        await authClient.verifyEmail(token);
        await refreshUser();
      }}
      renderLogo={() => <Brand />}
      renderAction={() => <Link to={user ? "/konto" : "/anmelden"}>{user ? "Zum Konto" : "Zur Anmeldung"}</Link>}
    />
  );
}
