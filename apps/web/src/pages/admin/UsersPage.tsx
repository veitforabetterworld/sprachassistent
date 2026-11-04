import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  VeitAlert,
  VeitButton,
  VeitEmptyState,
  VeitList,
  VeitListItem,
  VeitPage,
  VeitSettingsSection,
  VeitSettingsToggleRow,
  VeitSpinner,
  useVeitToast,
} from "@veit/react-controls";
import { VeitOverlayEditDialog, useDialogFormBaseline, useManagedOverlayDialogBinding } from "@veit/react-dialog";
import { adminApi, errorText, type AdminAccount } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

const nameOf = (user: AdminAccount) => `${user.first_name} ${user.last_name}`.trim() || user.email;

export function UsersPage() {
  const { t } = useI18n();
  const toast = useVeitToast();
  const [users, setUsers] = useState<AdminAccount[] | null>(null);
  const [required, setRequired] = useState(false);
  const [selected, setSelected] = useState<AdminAccount | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const data = await adminApi.users();
    setUsers(data.users);
    setRequired(data.verification_required);
  }, []);

  useEffect(() => {
    void load().catch((e: unknown) => setError(errorText(e)));
  }, [load]);

  const setVerification = async (enabled: boolean) => {
    try {
      setRequired((await adminApi.setVerificationRequired(enabled)).verification_required);
      toast.show("success", t("usersSettingSaved"));
    } catch (e) {
      toast.show("error", errorText(e));
    }
  };

  return (
    <VeitPage width="narrow">
      {error && <VeitAlert>{error}</VeitAlert>}
      <VeitSettingsSection title={t("usersTitle")}>
        <div className="flex flex-col gap-4">
          <VeitSettingsToggleRow
            variant="embed"
            label={t("usersVerification")}
            description={t("usersVerificationHint")}
            checked={required}
            disabled={users === null}
            onCheckedChange={(enabled) => void setVerification(enabled)}
          />
          {users === null && !error && <VeitSpinner label={t("loading")} />}
          {users?.length === 0 && <VeitEmptyState embedded title={t("usersEmpty")} />}
          {users && users.length > 0 && (
            <VeitList>
              {users.map((user) => (
                <VeitListItem
                  key={user.id}
                  title={nameOf(user)}
                  meta={`${user.email} · ${user.email_verified_at ? t("usersApproved") : t("usersOpen")}${user.is_admin ? ` · ${t("usersAdmin")}` : ""}`}
                  trailing={<ChevronRight className="size-4 text-muted-foreground" aria-hidden />}
                  onClick={() => setSelected(user)}
                />
              ))}
            </VeitList>
          )}
        </div>
      </VeitSettingsSection>
      <UserDialog user={selected} onClose={() => setSelected(null)} onChanged={() => void load()} />
    </VeitPage>
  );
}

/** One account: approve it (draft, saved in the footer), send a reset link, delete it in the footer. */
function UserDialog({ user, onClose, onChanged }: { user: AdminAccount | null; onClose: () => void; onChanged: () => void }) {
  const { t, tf } = useI18n();
  const toast = useVeitToast();
  const open = user !== null;
  const binding = useManagedOverlayDialogBinding(open, onClose);
  const initial = useMemo(() => ({ approved: user?.email_verified_at != null }), [user]);
  const [draft, setDraft] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (open) {
      setDraft(initial);
      setError(null);
    }
  }, [open, initial]);
  const { dirty, formBaseline } = useDialogFormBaseline({ draft, resolveBaseline: () => initial, resetDeps: [open, initial], track: open });

  /** Errors stay in the dialog and keep it open; the footer closes it after a resolved action. */
  const run = async (action: () => Promise<unknown>, done: string) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      toast.show("success", done);
      onChanged();
    } catch (e) {
      setError(errorText(e));
      throw e;
    } finally {
      setBusy(false);
    }
  };

  return (
    <VeitOverlayEditDialog
      binding={binding}
      title={user ? nameOf(user) : t("usersTitle")}
      closeAriaLabel={t("close")}
      variant="responsive"
      size="sm"
      disabled={busy}
      blockBackdropClose={busy}
      footerProps={{
        busy,
        dirty,
        formBaseline,
        cancelLabel: t("cancel"),
        saveLabel: t("save"),
        onSave: () => run(() => adminApi.approveUser(user!.id), t("usersApprovedNotice")),
        deleteAction: user
          ? {
              ariaLabel: t("delete"),
              confirm: {
                title: t("usersDeleteTitle"),
                message: tf("usersDeleteText", { name: nameOf(user) }),
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
                destructive: true,
              },
              onDelete: () => run(() => adminApi.deleteUser(user.id), t("usersDeleted")),
            }
          : undefined,
      }}
    >
      {user && (
        <div className="flex flex-col gap-4">
          <p className="m-0 text-sm text-muted-foreground">{user.email}</p>
          <VeitSettingsToggleRow
            variant="embed"
            label={t("usersApprovedLabel")}
            description={t("usersApprovedHint")}
            checked={draft.approved}
            // An approval cannot be taken back.
            disabled={busy || initial.approved}
            onCheckedChange={(approved) => setDraft({ approved })}
          />
          <div>
            <VeitButton
              variant="secondary"
              disabled={busy}
              onClick={() => void run(() => adminApi.resetUserPassword(user.id), t("usersResetSent")).catch(() => undefined)}
            >
              {t("usersReset")}
            </VeitButton>
          </div>
          {error && <VeitAlert>{error}</VeitAlert>}
        </div>
      )}
    </VeitOverlayEditDialog>
  );
}
