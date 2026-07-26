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

/**
 * Per-theme browser-chrome colors. Hex mirrors of the `--background`
 * HSL tokens in `web/styles/globals.css` — keep in sync when a theme's
 * background token changes.
 */
const THEME_COLORS: Record<ReturnType<typeof resolvedDataTheme>, string> = {
  dark: "#0c1012", // 222 22% 6%
  light: "#ffffff", // 0 0% 100%
  amber: "#f3efe7", // 38 35% 93%
};

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
 * Viewport for the reader route. The reader surface is intentionally
 * theme-independent black (`--reader-bg`, globals.css), so the
 * declared appearance is pinned to black/dark regardless of the
 * user's theme — the status-bar region then dresses dark instead of
 * showing a white system scrim over artwork when the chrome is
 * hidden.
 */
export const readerViewport: Viewport = {
  ...baseViewport,
  themeColor: "#000000",
  colorScheme: "dark",
};
