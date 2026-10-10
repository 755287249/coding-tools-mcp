import type { Locale } from './catalog';

export type LocalePreference = 'system' | Locale;
export const LOCALE_PREFERENCE_KEY = 'coding-tools.locale-preference';

/** Match the visitor/WebView language order, with script taking precedence over region. */
export function resolveSystemLocale(languages: readonly string[]): Locale {
  for (const language of languages) {
    const parts = language.trim().replaceAll('_', '-').toLowerCase().split('-');
    if (parts[0] === 'en') return 'en';
    if (parts[0] === 'ja') return 'ja';
    if (parts[0] === 'zh') {
      if (parts.includes('hant')) return 'zh-TW';
      if (parts.includes('hans')) return 'zh-CN';
      return parts.some(part => ['tw', 'hk', 'mo'].includes(part)) ? 'zh-TW' : 'zh-CN';
    }
  }
  return 'en';
}

export function loadLocalePreference(read: (key: string) => string | null): LocalePreference {
  const supported = ['en', 'zh-TW', 'zh-CN', 'ja'];
  try {
    const saved = read(LOCALE_PREFERENCE_KEY);
    if (saved === 'system' || (saved && supported.includes(saved))) return saved as LocalePreference;
    // The old subscriber wrote English even without a choice. New explicit
    // English choices use the preference key, so they survive every restart.
    const legacy = read('coding-tools.locale');
    if (legacy && legacy !== 'en' && supported.includes(legacy)) return legacy as Locale;
  } catch {
    // Browser privacy settings can deny storage without denying language access.
  }
  return 'system';
}
