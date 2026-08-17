/**
 * i18n: catalogue lookup with `{placeholder}` substitution, over a Preact
 * context.
 *
 * ## Why this is not a library
 *
 * See the dependency note in the commit message. The short version: this app is
 * embedded in the firmware image with `include_bytes!` and served from flash,
 * so the async resource loading that i18n libraries are built around — the
 * feature that justifies their size — is exactly the thing we cannot use. Both
 * catalogues are bundled, and the whole runtime need is "look up a key,
 * substitute a placeholder, re-render on switch".
 *
 * What a library would have bought (plural rules, dates, numbers, ICU
 * messages) is not needed by any string in `en.ts`. When it is — the first
 * time a message needs a real plural rule — reach for `Intl.PluralRules`,
 * which is already in every browser, before adding a dependency.
 *
 * The completeness guarantee is stronger than most libraries give: `fr.ts` is
 * typed `Record<MessageKey, string>`, so a missing translation is a build
 * failure rather than a runtime fallback to English.
 */
import { createContext, type ComponentChildren } from 'preact';
import { useCallback, useContext, useMemo, useState } from 'preact/hooks';

import { en, type MessageKey } from './en';
import { fr } from './fr';

export const LOCALES = ['en', 'fr'] as const;
export type Locale = (typeof LOCALES)[number];

/** Every locale must be a total catalogue — see `fr.ts`. */
const CATALOGUES: Record<Locale, Record<MessageKey, string>> = { en, fr };

export const LOCALE_NAMES: Record<Locale, string> = {
  en: 'English',
  fr: 'Français',
};

const STORAGE_KEY = 'somfy-rs.locale';

export type Translate = (key: MessageKey, params?: Record<string, string | number>) => string;

interface I18n {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: Translate;
}

const I18nContext = createContext<I18n | undefined>(undefined);

const isLocale = (value: string | null): value is Locale =>
  value !== null && (LOCALES as readonly string[]).includes(value);

/**
 * Stored choice first, then the browser's preference, then English. The
 * browser tag is matched on its primary subtag so `fr-CA` and `fr-BE` both
 * land on French.
 */
export function detectLocale(): Locale {
  const stored = safeRead();
  if (isLocale(stored)) return stored;
  for (const tag of navigator.languages ?? [navigator.language]) {
    const primary = tag.split('-')[0]?.toLowerCase();
    if (isLocale(primary ?? null)) return primary as Locale;
  }
  return 'en';
}

function safeRead(): string | null {
  // Private-browsing modes throw on `localStorage` access; a stored language
  // preference is never worth taking the app down for.
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

function safeWrite(locale: Locale): void {
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    /* preference simply will not persist */
  }
}

/** Replace every `{name}` in `message` with `params.name`. */
function interpolate(message: string, params?: Record<string, string | number>): string {
  if (!params) return message;
  return message.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = params[name];
    return value === undefined ? whole : String(value);
  });
}

export function I18nProvider({ children }: { children: ComponentChildren }) {
  const [locale, setLocaleState] = useState<Locale>(detectLocale);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    safeWrite(next);
    document.documentElement.lang = next;
  }, []);

  const t = useCallback<Translate>(
    (key, params) => interpolate(CATALOGUES[locale][key], params),
    [locale],
  );

  const value = useMemo<I18n>(() => ({ locale, setLocale, t }), [locale, setLocale, t]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18n {
  const value = useContext(I18nContext);
  if (!value) throw new Error('useI18n called outside <I18nProvider>');
  return value;
}

/** Shorthand for the common case of needing only `t`. */
export const useT = (): Translate => useI18n().t;
