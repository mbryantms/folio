import { getRequestConfig } from "next-intl/server";

// Mirror this list in the server's `auth/local.rs::SUPPORTED_LOCALES`.
// Adding a locale: extend this array, ship the message bundle, bump
// the server-side validator in lockstep.
export const SUPPORTED_LOCALES = ["en"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];
export const DEFAULT_LOCALE: Locale = "en";

// Post-Human-URLs M3: locale comes from the `NEXT_LOCALE` cookie (set on
// sign-in from `user.language`, or written by the language selector for
// unauthenticated visitors). `requestLocale` is populated by next-intl's
// proxy in `localePrefix: "never"` mode from cookie → Accept-Language.
export default getRequestConfig(async ({ requestLocale }) => {
  const requested = await requestLocale;
  const locale = SUPPORTED_LOCALES.includes(requested as Locale)
    ? (requested as Locale)
    : DEFAULT_LOCALE;
  return {
    locale,
    messages: (await import(`../messages/${locale}.json`)).default,
  };
});
