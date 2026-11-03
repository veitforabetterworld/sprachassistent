import { useI18n } from "../../i18n/I18nProvider";

/** What the other side said behind an error, folded away under our own sentence. */
export function ErrorDetails({ detail }: { detail?: string }) {
  const { t } = useI18n();
  if (!detail) return null;
  return (
    <details className="text-xs text-muted-foreground">
      <summary className="cursor-pointer select-none">{t("errDetails")}</summary>
      <pre className="m-0 mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-muted px-2 py-1">{detail}</pre>
    </details>
  );
}
