import { useNavigate } from "react-router-dom";
import { VeitAccountSettings, veitAccountTexts } from "@veit/react-account";
import { veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import { VeitPage } from "@veit/react-controls";
import { accountClient, authClient, useAccount } from "../api";
import { useI18n } from "../i18n/I18nProvider";
import { SignedIn } from "./admin/AdminLayout";

/** Profile, password, two-factor login and sessions: the account page of the framework. */
export function AccountPage() {
  return (
    <SignedIn>
      <AccountSettings />
    </SignedIn>
  );
}

function AccountSettings() {
  const { locale } = useI18n();
  const { user, refreshUser } = useAccount();
  const navigate = useNavigate();
  const authT = veitAuthT(locale);
  if (!user) return null;

  return (
    <VeitPage width="narrow">
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
    </VeitPage>
  );
}
