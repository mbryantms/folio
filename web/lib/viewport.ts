import type { Viewport } from "next";
import { resolvedDataTheme, type Theme } from "@/lib/theme";

/**
 * Shared viewport configuration.
 *
 * Explicit viewport with pinch-zoom enabled. Without this, Next's
 * default omits `maximum-scale` / `userScalable`, but some embeds
 * and some PWA installs still end up at scale=1 only. Pinning the
 * values explicitly guarantees mobile users can pinch-zoom anywhere
 * in the app to read small text on series/issue cards, the admin
 * tables, and OPDS pages. The reader (Reader.tsx) opts back into
 * native pinch-zoom by setting `touch-action: pan-y pinch-zoom`
 * on its container — its drag handler ignores the swipe when
 * `visualViewport.scale > 1` so panning a zoomed page doesn't
 * accidentally turn the page.
 *
 * `viewportFit: "cover"` lets the app paint into the area behind
 * the iOS notch / Dynamic Island. Interactive elements that need
 * to stay clear of the inset (the topbar in particular) read the
 * `env(safe-area-inset-*)` CSS variables from their own padding;
 * the body itself is allowed to extend full-bleed.
 */
export const baseViewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  maximumScale: 5,
  userScalable: true,
  viewportFit: "cover",
};

import { THEME_COLORS } from "./pwa/theme-colors";

/** One `<meta name="theme-color">` (optionally media-scoped). */
export interface ThemeColorMeta {
  media?: string;
  color: string;
}

/**
 * The `theme-color` + `color-scheme` metas for the user's cookie-resolved
 * theme. Rendered as STATIC tags in the root layout's <head> (see
 * `app/layout.tsx`), NOT through Next's metadata/viewport API.
 *
 * Why: Next re-renders its whole metadata tree on every client-side
 * navigation — every meta it manages (theme-color, color-scheme, the
 * apple-mobile-web-app-* tags, viewport…) is removed from <head> and
 * re-inserted, even when nothing changed. An installed iPadOS app treats
 * that as a runtime change and latches it: the status-bar strip flips
 * from the top bar's solid colour to a blur of the scrolled content and
 * stays that way, on every route, until the app is force-quit (seen on
 * 26.1, reproduced on 27.0.1). Tags rendered by the root layout itself
 * persist across navigations, so iOS never sees them change.
 *
 * `themeColor` follows the cookie theme, not `prefers-color-scheme`: a
 * dark-themed app on a light-mode device would otherwise declare itself
 * white. Only an explicit `system` choice uses the media-query pair.
 */
export function themeHeadMeta(theme: Theme): {
  colorScheme: string;
  themeColor: ThemeColorMeta[];
} {
  if (theme === "system") {
    return {
      colorScheme: "dark light",
      themeColor: [
        { media: "(prefers-color-scheme: dark)", color: THEME_COLORS.dark },
        { media: "(prefers-color-scheme: light)", color: THEME_COLORS.light },
      ],
    };
  }
  const resolved = resolvedDataTheme(theme);
  return {
    colorScheme: resolved === "dark" ? "dark" : "light",
    themeColor: [{ color: THEME_COLORS[resolved] }],
  };
}

// There is deliberately no reader-specific viewport or theme-color: the
// reader used to pin black for light / amber / system themes (#541),
// which changed these tags on every navigation into it — the latch
// described above. On iPadOS the strip's colour comes from the top-edge
// sticky/fixed container, not theme-color, so the reader loses nothing.
