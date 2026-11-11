/**
 * Standalone-mode detection. Covers both the modern
 * `display-mode: standalone` media query (every platform that
 * honours the web app manifest's `display` field) and Safari's
 * legacy `navigator.standalone` boolean (still the most reliable
 * signal on iOS, where the manifest is partially honored).
 *
 * Its own module (re-exported from `lib/use-pull-to-refresh.ts`) because
 * the root layout's `SafeAreaProbe` and `InstallEvents` need only this
 * check, and importing it from the pull-to-refresh hook put that whole
 * module in every page's first load (reader bundle budget,
 * `docs/dev/pwa-performance.md`).
 */
export function isStandaloneDisplay(): boolean {
  if (typeof window === "undefined") return false;
  if (window.matchMedia("(display-mode: standalone)").matches) return true;
  const nav = window.navigator as Navigator & { standalone?: boolean };
  return nav.standalone === true;
}
