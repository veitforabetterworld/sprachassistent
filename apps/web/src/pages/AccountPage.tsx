import { Navigate, useNavigate } from "react-router-dom";
import { VeitAccountSettings, veitAccountTexts } from "@veit/react-account";
import { veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import { AppShell, VeitSpinner } from "@veit/react-controls";
import { accountClient, authClient, useAccount } from "../api";
import { useI18n } from "../i18n/I18nProvider";
import { Layout } from "./Layout";

/** Profile, password, two-factor login and sessions: the account page of the framework. */
export function AccountPage() {
  const { t, locale } = useI18n();
  const { user, loading, refreshUser } = useAccount();
  const navigate = useNavigate();
  const authT = veitAuthT(locale);

  if (loading) {
    return (
      <AppShell variant="center">
        <VeitSpinner size="lg" label={t("loading")} />
      </AppShell>
    );
  }
  if (!user) return <Navigate to="/anmelden" replace />;

  return (
    <Layout>
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
        <VeitAccountSettings
          client={accountClient}
          user={{
            firstName: user.first_name,
            lastName: user.last_name,
            email: user.email,
            emailVerified: user.email_verified_at !== null,
          }}
          texts={veitAccountTexts[locale]}
          errorText={(error: unknown) => veitAuthErrorText(error, authT)}
          onUserChanged={() => void refreshUser()}
          onDeleted={() => {
            void refreshUser();
            navigate("/anmelden", { replace: true });
          }}
          resendVerification={() => authClient.resendVerification()}
        />
      </div>
    </Layout>
  );
}
