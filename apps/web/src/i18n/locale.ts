export type UiLocale = "de" | "en";

let active: UiLocale = "de";

export function getActiveLocale(): UiLocale {
  return active;
}

export function setActiveLocale(locale: UiLocale): void {
  active = locale;
}

export function readStoredLocale(): UiLocale {
  if (typeof localStorage === "undefined") return "de";
  return localStorage.getItem("sprachassistent-locale") === "en" ? "en" : "de";
}

export function storeLocale(locale: UiLocale): void {
  localStorage.setItem("sprachassistent-locale", locale);
}
