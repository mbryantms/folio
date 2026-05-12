/**
 * Theme + accent + density token shapes (M4).
 *
 * Persisted in two places:
 *   - cookie (`comic_theme`, `comic_accent`, `comic_density`) for FOUC-free
 *     SSR — read on the server before paint and applied to <html>
 *   - users.theme / users.accent_color / users.density (Postgres) for
 *     cross-device + cross-browser persistence — written via debounced
 *     PATCH /me/preferences after every change
 *
 * Cookies are non-HttpOnly so the client can read them on first paint and
 * keep them in sync with next-themes. They are not security tokens; the DB
 * row is authoritative.
 */

export type Theme = "system" | "dark" | "light" | "amber";
export type Accent = "amber" | "blue" | "emerald" | "rose";
export type Density = "comfortable" | "compact";

export const THEMES: readonly Theme[] = [
  "system",
  "dark",
  "light",
  "amber",
] as const;
export const ACCENTS: readonly Accent[] = [
  "amber",
  "blue",
  "emerald",
  "rose",
] as const;
export const DENSITIES: readonly Density[] = [
  "comfortable",
  "compact",
] as const;

export const THEME_COOKIE = "comic_theme";
export const ACCENT_COOKIE = "comic_accent";
export const DENSITY_COOKIE = "comic_density";

/** 1 year — long enough that the FOUC-free path is reliable across visits. */
const COOKIE_MAX_AGE = 60 * 60 * 24 * 365;

/**
 * `system` resolves to `dark` for now because we don't ship a curated light
 * palette in v1. `amber` collapses to `dark` at the data-theme level — the
 * accent picker handles the amber tinting separately.
 */
export function resolvedDataTheme(
  theme: Theme | null | undefined,
): "dark" | "light" {
  if (theme === "light") return "light";
  return "dark";
}

export function isTheme(v: unknown): v is Theme {
  return v === "system" || v === "dark" || v === "light" || v === "amber";
}
export function isAccent(v: unknown): v is Accent {
  return v === "amber" || v === "blue" || v === "emerald" || v === "rose";
}
export function isDensity(v: unknown): v is Density {
  return v === "comfortable" || v === "compact";
}

export function writeThemeCookie(theme: Theme | null) {
  writeCookie(THEME_COOKIE, theme);
}
export function writeAccentCookie(accent: Accent | null) {
  writeCookie(ACCENT_COOKIE, accent);
}
export function writeDensityCookie(density: Density | null) {
  writeCookie(DENSITY_COOKIE, density);
}

function writeCookie(name: string, value: string | null) {
  if (typeof document === "undefined") return;
  if (value === null) {
    document.cookie = `${name}=; Path=/; Max-Age=0; SameSite=Lax`;
    return;
  }
  document.cookie = `${name}=${encodeURIComponent(value)}; Path=/; Max-Age=${COOKIE_MAX_AGE}; SameSite=Lax`;
}
