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

/**
 * Viewport for the user's actual (cookie-resolved) theme.
 *
 * `themeColor` drives the iOS/iPadOS status-bar dressing in standalone
 * mode and the Android browser chrome color. It must track the app's
 * cookie-driven theme, not `prefers-color-scheme` — a dark-themed app
 * on a light-mode device otherwise declares itself white, and iPadOS
 * paints a white status-bar backing over dark content (the reader was
 * the flagrant case). Only an explicit `theme=system` choice falls
 * back to the OS-preference media-query pair, because the server
 * can't observe the client's preference.
 *
 * `colorScheme` emits `<meta name="color-scheme">`, which is what
 * WebKit consults to classify the page as dark or light content when
 * dressing system chrome around the web view.
 */
export function themedViewport(theme: Theme): Viewport {
  if (theme === "system") {
    return {
      ...baseViewport,
      themeColor: [
        { media: "(prefers-color-scheme: dark)", color: THEME_COLORS.dark },
        { media: "(prefers-color-scheme: light)", color: THEME_COLORS.light },
      ],
      colorScheme: "dark light",
    };
  }
  const resolved = resolvedDataTheme(theme);
  return {
    ...baseViewport,
    themeColor: THEME_COLORS[resolved],
    colorScheme: resolved === "dark" ? "dark" : "light",
  };
}

// There is deliberately no reader-specific viewport. The reader used to
// pin `theme-color: #000000` + `color-scheme: dark` for light / amber /
// system themes (#541: a white status-bar tint over artwork), returning
// the root viewport only for an explicit dark theme. Every navigation
// into the reader then rewrote those <meta> tags at runtime, and an
// installed iPadOS app LATCHES the first such change: the status-bar
// strip switches from the header's solid colour to a blur of the page
// content and stays that way on every route until the app is force-quit
// (observed on iPadOS 26.1, reproduced on 27.0.1 with theme = system
// while the OS was in light mode). Since iPadOS 26 the strip's colour
// comes from the top-edge sticky/fixed container, not theme-color, so the
// reader loses nothing there; Android reader chrome follows the theme
// colour instead of black, which is the lesser evil.
