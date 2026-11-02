import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { ForgotPasswordForm, LoginForm, RegisterForm, ResetPasswordForm, VerifyEmailView, veitAuthErrorText, veitAuthT, veitFetchJson } from "@veit/react-auth";
import { VeitSettingsToggleRow } from "@veit/react-controls";
import { authClient, useAccount, type AccountUser } from "../api";
import { useI18n } from "../i18n/I18nProvider";
import { linkClass, LocaleSwitch } from "./Layout";

/** Pages around the account forms of @veit/react-auth; texts come from the framework, links and brand from here. */

const AFTER_LOGIN = "/admin";

function useAuthTexts() {
  const { locale } = useI18n();
  return useMemo(() => {
    const t = veitAuthT(locale);
    return { t, parseError: (error: unknown) => veitAuthErrorText(error, t) };
  }, [locale]);
}

function Brand() {
  const { t } = useI18n();
  return (
    <div className="flex items-center justify-between gap-3">
      <strong className="text-xl font-bold">{t("appName")}</strong>
      <LocaleSwitch />
    </div>
  );
}

function FooterLink({ to, label }: { to: string; label: string }) {
  return (
    <p className="mt-4 text-center text-sm">
      <Link to={to} className={linkClass}>
        {label}
      </Link>
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
  const { t, parseError } = useAuthTexts();
  const { t: tr } = useI18n();
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
          {registrationOpen && <FooterLink to="/registrieren" label={tr("registerFirstAdmin")} />}
        </>
      )}
    />
  );
}

export function RegisterPage() {
  const { t, parseError } = useAuthTexts();
  const { t: tr } = useI18n();
  return (
    <RegisterForm<AccountUser>
      t={t}
      parseError={parseError}
      afterRegisterPath={AFTER_LOGIN}
      onSuggestPassword={() => authClient.suggestPassword()}
      renderLogo={() => <Brand />}
      renderPrivacyConsent={(checked, onCheckedChange) => (
        <VeitSettingsToggleRow variant="embed" label={tr("privacyConsent")} checked={checked} onCheckedChange={onCheckedChange} />
      )}
      renderFooter={() => <FooterLink to="/anmelden" label={tr("haveAccount")} />}
    />
  );
}

export function ForgotPasswordPage() {
  const { t, parseError } = useAuthTexts();
  const { t: tr } = useI18n();
  return (
    <ForgotPasswordForm<AccountUser>
      t={t}
      parseError={parseError}
      onRequestReset={(email) => authClient.requestPasswordReset!(email)}
      renderLogo={() => <Brand />}
      renderFooter={() => <FooterLink to="/anmelden" label={tr("backToLogin")} />}
    />
  );
}

export function ResetPasswordPage() {
  const { t, parseError } = useAuthTexts();
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
  const { t } = useAuthTexts();
  const { t: tr } = useI18n();
  const { user, refreshUser } = useAccount();
  return (
    <VerifyEmailView
      t={t}
      onVerify={async (token) => {
        await authClient.verifyEmail(token);
        await refreshUser();
      }}
      renderLogo={() => <Brand />}
      renderAction={() => (
        <Link to={user ? "/konto" : "/anmelden"} className={linkClass}>
          {user ? tr("toAccount") : tr("toLogin")}
        </Link>
      )}
    />
  );
}
