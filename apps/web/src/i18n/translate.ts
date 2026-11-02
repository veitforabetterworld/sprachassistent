import { de, type MessageKey } from "./de";
import { en } from "./en";
import { getActiveLocale } from "./locale";

const catalogs = { de, en };

/**
 * Text in the active locale for code outside React components (formatters, map setup). Components use `t`
 * from `useI18n`, which re-renders on a locale switch; callers of `tr` run during that render.
 */
export function tr(key: MessageKey): string {
  return catalogs[getActiveLocale()][key];
}

/** Text with `{name}` placeholders filled in. */
export function trFormat(key: MessageKey, values: Record<string, string | number>): string {
  return fillPlaceholders(tr(key), values);
}

export function fillPlaceholders(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (match, name: string) => (name in values ? String(values[name]) : match));
}
