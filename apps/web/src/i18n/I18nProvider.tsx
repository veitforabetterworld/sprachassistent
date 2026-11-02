import { formatLocalizedDateTime } from "@veit/intl";
import { createContext, useContext, useMemo, useState, type ReactNode } from "react";
import { de, type MessageKey } from "./de";
import { en } from "./en";
import { readStoredLocale, setActiveLocale, storeLocale, type UiLocale } from "./locale";
import { fillPlaceholders } from "./translate";

const catalogs = { de, en };

type I18nApi = {
  locale: UiLocale;
  setLocale: (locale: UiLocale) => void;
  t: (key: MessageKey) => string;
  /** Text with `{name}` placeholders filled in. */
  tf: (key: MessageKey, values: Record<string, string | number>) => string;
  /** Date and time of an ISO timestamp in the active locale. */
  formatTime: (iso: string) => string;
};

const I18nContext = createContext<I18nApi | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<UiLocale>(() => readStoredLocale());
  setActiveLocale(locale);
  const api = useMemo<I18nApi>(
    () => ({
      locale,
      setLocale: (next) => {
        storeLocale(next);
        setActiveLocale(next);
        setLocaleState(next);
      },
      t: (key) => catalogs[locale][key],
      tf: (key, values) => fillPlaceholders(catalogs[locale][key], values),
      formatTime: (iso) => formatLocalizedDateTime(locale, iso, { dateStyle: "short", timeStyle: "short" }),
    }),
    [locale],
  );
  return <I18nContext.Provider value={api}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nApi {
  const api = useContext(I18nContext);
  if (!api) throw new Error("useI18n außerhalb von I18nProvider"); // i18n-ignore: developer error
  return api;
}
