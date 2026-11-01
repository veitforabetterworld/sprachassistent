import { Link, Navigate, useNavigate } from "react-router-dom";
import { VeitAccountSettings, veitAccountTexts } from "@veit/react-account";
import { veitAuthErrorText, veitAuthT } from "@veit/react-auth";
import { VeitButton, VeitSpinner } from "@veit/react-controls";
import { accountClient, authClient, useAccount } from "../api";

const authT = veitAuthT("de");
const errorText = (error: unknown) => veitAuthErrorText(error, authT);

/** Profile, password, two-factor login and sessions: the account page of the framework. */
export function AccountPage() {
  const { user, loading, refreshUser, logout } = useAccount();
  const navigate = useNavigate();

  if (loading) {
    return (
      <div className="flex justify-center py-16">
        <VeitSpinner size="lg" label="Lädt" />
      </div>
    );
  }
  if (!user) return <Navigate to="/anmelden" replace />;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="m-0 text-2xl font-bold">Konto</h2>
        <div className="flex gap-2">
          {user.is_admin && (
            <Link className="btn-secondary btn-sm no-underline" to="/admin">
              Adminbereich
            </Link>
          )}
          <VeitButton
            type="button"
            variant="secondary"
            className="btn-sm"
            onClick={() => void logout().then(() => navigate("/anmelden", { replace: true }))}
          >
            Abmelden
          </VeitButton>
        </div>
      </div>
      <VeitAccountSettings
        client={accountClient}
        user={{
          firstName: user.first_name,
          lastName: user.last_name,
          email: user.email,
          emailVerified: user.email_verified_at !== null,
        }}
        texts={veitAccountTexts.de}
        errorText={errorText}
        onUserChanged={() => void refreshUser()}
        onDeleted={() => {
          void refreshUser();
          navigate("/anmelden", { replace: true });
        }}
        resendVerification={() => authClient.resendVerification()}
      />
    </div>
  );
}
