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

/**
 * Viewport for the reader route.
 *
 * The reader surface is theme-independent black, so on a light/amber
 * theme its declared appearance is pinned to black/dark — otherwise the
 * status-bar region dresses white over the artwork (#541).
 *
 * For a dark theme the reader returns EXACTLY the root
 * viewport instead of `#000000`. Since iOS/iPadOS 26.1 the OS paints its
 * own opaque status bar (plus a short fade below it) from `theme-color`,
 * and it latches the first runtime change: entering the reader turned the
 * fade black, and leaving it — although the DOM reverts to the theme
 * colour — kept the black fade on every page until the app was force-
 * quit. Keeping the meta byte-identical across the navigation removes
 * the trigger for the common case; light/amber users still get the black
 * reader dressing (the alternative is a white fade over artwork).
 */
export function readerViewport(theme: Theme): Viewport {
  if (theme === "system") {
    return {
      ...themedViewport(theme),
      themeColor: [
        { media: "(prefers-color-scheme: dark)", color: THEME_COLORS.dark },
        { media: "(prefers-color-scheme: light)", color: "#000000" },
      ],
      colorScheme: "dark",
    };
  }
  if (resolvedDataTheme(theme) === "dark") {
    return themedViewport(theme);
  }
  return {
    ...baseViewport,
    themeColor: "#000000",
    colorScheme: "dark",
  };
}
