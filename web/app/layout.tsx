import type { Metadata, Viewport } from "next";
import { NextIntlClientProvider } from "next-intl";
import { getLocale, getMessages } from "next-intl/server";
import { cookies } from "next/headers";
import { GlobalHotkeys } from "@/components/GlobalHotkeys";
import { GlobalShortcutsSheet } from "@/components/GlobalShortcutsSheet";
import { QueryProvider } from "@/components/QueryProvider";
import { ScanResultListener } from "@/components/ScanResultListener";
import { ServiceWorkerUpdater } from "@/components/ServiceWorkerUpdater";
import { ThemeProvider } from "@/components/ThemeProvider";
import { Toaster } from "@/components/ui/sonner";
import {
  ACCENT_COOKIE,
  DENSITY_COOKIE,
  THEME_COOKIE,
  isAccent,
  isDensity,
  isTheme,
  resolvedDataTheme,
} from "@/lib/theme";
import "@/styles/globals.css";

// Apple touch startup images. The `media` query is what binds an
// image file to a specific iOS device resolution and orientation;
// iOS picks the first matching entry. Sizes here cover the modern
// iPhone lineup (12-mini through 15 Pro Max) plus the standard
// iPad / iPad Air / iPad Pro families. Older devices fall back to
// the plain `theme_color` splash. Files are produced by
// `pwa-asset-generator --splash-only` per the icons README.
const APPLE_STARTUP_IMAGES = [
  // iPhone 14 Pro Max / 15 Plus / 15 Pro Max  — 1290 x 2796 (@3x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1290x2796.png", media: "(device-width: 430px) and (device-height: 932px) and (-webkit-device-pixel-ratio: 3) and (orientation: portrait)" },
  // iPhone 14 Pro / 15 / 15 Pro  — 1179 x 2556 (@3x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1179x2556.png", media: "(device-width: 393px) and (device-height: 852px) and (-webkit-device-pixel-ratio: 3) and (orientation: portrait)" },
  // iPhone 12/13/14, 12/13 Pro, 14 Plus  — 1170 x 2532 (@3x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1170x2532.png", media: "(device-width: 390px) and (device-height: 844px) and (-webkit-device-pixel-ratio: 3) and (orientation: portrait)" },
  // iPhone 12/13 mini  — 1080 x 2340 (@3x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1080x2340.png", media: "(device-width: 360px) and (device-height: 780px) and (-webkit-device-pixel-ratio: 3) and (orientation: portrait)" },
  // iPhone XR / 11  — 828 x 1792 (@2x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-828x1792.png", media: "(device-width: 414px) and (device-height: 896px) and (-webkit-device-pixel-ratio: 2) and (orientation: portrait)" },
  // iPhone X / XS / 11 Pro  — 1125 x 2436 (@3x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1125x2436.png", media: "(device-width: 375px) and (device-height: 812px) and (-webkit-device-pixel-ratio: 3) and (orientation: portrait)" },
  // iPad Pro 12.9"  — 2048 x 2732 (@2x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-2048x2732.png", media: "(device-width: 1024px) and (device-height: 1366px) and (-webkit-device-pixel-ratio: 2) and (orientation: portrait)" },
  // iPad Pro 11" / Air  — 1668 x 2388 (@2x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1668x2388.png", media: "(device-width: 834px) and (device-height: 1194px) and (-webkit-device-pixel-ratio: 2) and (orientation: portrait)" },
  // iPad / iPad mini  — 1536 x 2048 (@2x)
  { rel: "apple-touch-startup-image", url: "/icons/splash-1536x2048.png", media: "(device-width: 768px) and (device-height: 1024px) and (-webkit-device-pixel-ratio: 2) and (orientation: portrait)" },
];

export const metadata: Metadata = {
  title: "Folio",
  description: "Self-hostable comic reader",
  // Apple-specific PWA tags. `capable: true` emits
  // `<meta name="apple-mobile-web-app-capable" content="yes">`,
  // which is the legacy-iOS opt-in to standalone launch and the
  // signal `usePullToRefresh` reads via `navigator.standalone`.
  // The `black-translucent` status bar style lets the app paint
  // under the iOS status bar; the dark `theme_color` in
  // `manifest.ts` keeps the area readable.
  appleWebApp: {
    capable: true,
    title: "Folio",
    statusBarStyle: "black-translucent",
  },
  // Apple touch icon. Required for iOS to use a real branded icon
  // when the app is added to the Home Screen — without it, iOS
  // takes a screenshot of the page (usually ugly). The file must
  // exist at `web/public/icons/apple-touch-icon.png` (180×180 PNG).
  //
  // The `other` array configures `apple-touch-startup-image`s,
  // which iOS uses for the splash screen between Home Screen tap
  // and first paint when the app is launched in standalone mode.
  // Each device-class needs its own file at the exact device
  // resolution — `pwa-asset-generator --splash-only` emits the
  // full set in one pass. See `web/public/icons/README.md`. When
  // a file is missing, iOS falls back to a plain `theme_color`
  // splash (`black-translucent` over `#0c1012`) which is fine
  // but unbranded.
  icons: {
    apple: "/icons/apple-touch-icon.png",
    other: APPLE_STARTUP_IMAGES,
  },
};

/**
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
 *
 * `themeColor` drives the iOS status bar tint and the Android
 * browser chrome color. Light and dark variants are emitted as
 * separate `<meta name="theme-color">` tags with `media` queries.
 * The hex values mirror the dark and light `--background` tokens
 * from `web/styles/globals.css`.
 */
export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  maximumScale: 5,
  userScalable: true,
  viewportFit: "cover",
  themeColor: [
    { media: "(prefers-color-scheme: dark)", color: "#0c1012" },
    { media: "(prefers-color-scheme: light)", color: "#ffffff" },
  ],
};

// Post-Human-URLs M3: locale is no longer a route param. Read it via
// `getLocale()` from next-intl/server, which resolves cookie/header per
// the proxy config (`localePrefix: "never"`).
export default async function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  const locale = await getLocale();
  const messages = await getMessages();

  // Read theme/accent/density cookies server-side so the first paint already
  // has the user's choice — avoids the "dark flash to light" FOUC.
  const jar = await cookies();
  const themeCookie = jar.get(THEME_COOKIE)?.value;
  const accentCookie = jar.get(ACCENT_COOKIE)?.value;
  const densityCookie = jar.get(DENSITY_COOKIE)?.value;
  const theme = isTheme(themeCookie) ? themeCookie : "dark";
  const accent = isAccent(accentCookie) ? accentCookie : "amber";
  const density = isDensity(densityCookie) ? densityCookie : "comfortable";
  const dataTheme = resolvedDataTheme(theme);

  return (
    <html
      lang={locale}
      className="h-full"
      data-theme={dataTheme}
      data-accent={accent}
      data-density={density}
      suppressHydrationWarning
    >
      <body className="bg-background text-foreground min-h-full antialiased">
        <ThemeProvider defaultTheme={theme}>
          <NextIntlClientProvider messages={messages}>
            <QueryProvider>
              <ServiceWorkerUpdater>
                <ScanResultListener />
                <GlobalHotkeys />
                <GlobalShortcutsSheet>{children}</GlobalShortcutsSheet>
              </ServiceWorkerUpdater>
            </QueryProvider>
          </NextIntlClientProvider>
          <Toaster />
        </ThemeProvider>
      </body>
    </html>
  );
}
