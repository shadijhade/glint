import { AR } from "./i18n-ar.ts";

/** The languages Glint's interface speaks; English is its own text, the others are catalogs of it. */
export type Language = "en" | "ar";
export type LanguageSetting = "system" | Language;

export const LANGUAGES: [LanguageSetting, string][] = [
    ["system", "Windows' language"],
    ["en", "English"],
    ["ar", "العربية"],
];

const CATALOGS: Record<Language, Record<string, string>> = { en: {}, ar: AR };
/** Written right to left. */
const RTL = new Set<Language>(["ar"]);

let current: Language = "en";

/** The language for a setting: the one chosen, else Windows' own where Glint speaks it, else English. */
export function languageOf(setting: string | undefined): Language {
    if (setting === "en" || setting === "ar") return setting;
    const asked = (navigator.languages?.length ? navigator.languages : [navigator.language]).map((l) => l.toLowerCase());
    return asked.some((l) => l.startsWith("ar")) ? "ar" : "en";
}

/** Speaks `language` from now on: the page's lang and direction (right to left for Arabic) follow. */
export function setLanguage(language: Language) {
    current = language;
    document.documentElement.lang = language;
    document.documentElement.dir = RTL.has(language) ? "rtl" : "ltr";
}

export const language = () => current;
export const isRtl = () => RTL.has(current);

/**
 * `text` in the interface's language: English is the key, and what a catalog doesn't have stays
 * English. `{name}` parts are filled from `vars` after the lookup, so a catalog keeps them.
 */
export function t(text: string, vars?: Record<string, string | number>): string {
    let out = CATALOGS[current][text] ?? text;
    if (vars) for (const [name, value] of Object.entries(vars)) out = out.split(`{${name}}`).join(String(value));
    return out;
}

/** What `t` would give in `language`, for checks. */
export function translate(language: Language, text: string) {
    return CATALOGS[language][text] ?? text;
}
