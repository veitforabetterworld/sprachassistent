import { useCallback, useEffect, useState } from "react";
import { ChevronRight } from "lucide-react";
import { VeitButton, VeitDeleteButton, VeitEmptyState, VeitList, VeitListItem, VeitSettingsSection, VeitSettingsToggleRow, VeitStatusBadge } from "@veit/react-controls";
import { useManagedOverlayDialogBinding, VeitOverlayDialog } from "@veit/react-dialog";
import { adminApi, errorText, type AdminAccount } from "../../api";
import { useI18n } from "../../i18n/I18nProvider";

type Notice = { ok: boolean; text: string } | null;

export function UsersPage() {
  const { t, tf } = useI18n();
  const [users, setUsers] = useState<AdminAccount[] | null>(null);
  const [required, setRequired] = useState(false);
  const [selected, setSelected] = useState<AdminAccount | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const [busy, setBusy] = useState(false);
  const binding = useManagedOverlayDialogBinding(selected !== null, () => setSelected(null));

  const load = useCallback(async () => {
    const data = await adminApi.users();
    setUsers(data.users);
    setRequired(data.verification_required);
    setSelected((current) => data.users.find((user) => user.id === current?.id) ?? null);
  }, []);

  useEffect(() => {
    void load().catch((error: unknown) => setNotice({ ok: false, text: errorText(error) }));
  }, [load]);

  const run = async (action: () => Promise<void>, ok: string) => {
    setBusy(true);
    try {
      await action();
      await load();
      setNotice({ ok: true, text: ok });
    } catch (error) {
      setNotice({ ok: false, text: errorText(error) });
    } finally {
      setBusy(false);
    }
  };

  const nameOf = (user: AdminAccount) => `${user.first_name} ${user.last_name}`.trim() || user.email;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-4 sm:p-6">
      {notice && (
        <VeitStatusBadge variant={notice.ok ? "success" : "destructive"} shape="rounded">
          {notice.text}
        </VeitStatusBadge>
      )}
      <VeitSettingsSection title={t("usersTitle")}>
        <VeitSettingsToggleRow
          variant="embed"
          label={t("usersVerification")}
          description={t("usersVerificationHint")}
          checked={required}
          disabled={users === null}
          onCheckedChange={(enabled) =>
            void run(async () => {
              setRequired((await adminApi.setVerificationRequired(enabled)).verification_required);
            }, t("usersSettingSaved"))
          }
        />
        {users?.length === 0 && <VeitEmptyState title={t("usersEmpty")} />}
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
      </VeitSettingsSection>
      <VeitOverlayDialog binding={binding} title={selected ? nameOf(selected) : t("usersTitle")} closeAriaLabel={t("close")} variant="responsive" size="sm">
        {selected && (
          <div className="flex flex-col gap-3">
            <p className="m-0 text-sm text-muted-foreground">{selected.email}</p>
            {!selected.email_verified_at && (
              <VeitButton disabled={busy} onClick={() => void run(() => adminApi.approveUser(selected.id).then(() => undefined), t("usersApprovedNotice"))}>
                {t("usersApprove")}
              </VeitButton>
            )}
            <VeitButton variant="secondary" disabled={busy} onClick={() => void run(() => adminApi.resetUserPassword(selected.id).then(() => undefined), t("usersResetSent"))}>
              {t("usersReset")}
            </VeitButton>
            <VeitDeleteButton
              disabled={busy}
              deleteConfirm={{
                title: t("usersDeleteTitle"),
                message: tf("usersDeleteText", { name: nameOf(selected) }),
                confirmLabel: t("delete"),
                cancelLabel: t("cancel"),
                destructive: true,
              }}
              onClick={() =>
                void run(async () => {
                  await adminApi.deleteUser(selected.id);
                  setSelected(null);
                }, t("usersDeleted"))
              }
            >
              {t("delete")}
            </VeitDeleteButton>
          </div>
        )}
      </VeitOverlayDialog>
    </div>
  );
}
