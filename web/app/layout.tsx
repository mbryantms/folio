import type { Metadata, Viewport } from "next";
import { baseViewport, themeHeadMeta } from "@/lib/viewport";
import { appleStartupImages } from "@/lib/pwa/apple-splash";
import { NextIntlClientProvider } from "next-intl";
import { getLocale, getMessages } from "next-intl/server";
import { cookies, headers } from "next/headers";
import { GlobalHotkeys } from "@/components/GlobalHotkeys";
import { HydrateAuthCache } from "@/components/HydrateAuthCache";
import { getMe, SESSION_COOKIE } from "@/lib/api/me";
import { SearchModalProvider } from "@/lib/search/use-search-modal";
import { GlobalShortcutsSheet } from "@/components/GlobalShortcutsSheet";
import { QueryProvider } from "@/components/QueryProvider";
import { ScanResultListener } from "@/components/ScanResultListener";
import { ServiceWorkerLoader } from "@/components/ServiceWorkerLoader";
import { VisualViewportSync } from "@/components/VisualViewportSync";
import { InstallEvents } from "@/components/InstallEvents";
import { SafeAreaProbe } from "@/components/SafeAreaProbe";
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

export const metadata: Metadata = {
  title: "Folio",
  description: "Self-hostable comic reader",
  // The Apple PWA tags (apple-mobile-web-app-*), theme-color and
  // color-scheme are NOT declared here on purpose: Next re-renders every
  // metadata-API tag on each client navigation (remove + re-insert), and
  // an installed iPadOS app latches that into a permanently blurred
  // status-bar strip. They are static <head> children of RootLayout
  // below, which persists across navigations. See lib/viewport.ts.
  // Every file below is generated from `public/brand/icon-master.svg` by
  // `pnpm --filter web run build-icons` (see `public/icons/README.md`).
  //
  // - `icon`: tab favicon. The multi-size `.ico` for legacy browsers and
  //   the SVG for everything modern.
  // - `apple`: iOS Home Screen icon (180×180, opaque). Without it iOS
  //   takes a screenshot of the page as the icon.
  // - `other`: `apple-touch-startup-image`s, the splash iOS shows between
  //   a Home Screen tap and first paint in standalone mode. One file per
  //   device class and orientation at the exact device resolution; see
  //   `lib/pwa/apple-splash.ts`.
  icons: {
    icon: [
      { url: "/favicon.ico", sizes: "16x16 32x32 48x48" },
      { url: "/icon.svg", type: "image/svg+xml" },
    ],
    apple: { url: "/icons/apple-touch-icon.png", sizes: "180x180" },
    other: appleStartupImages(),
  },
};

/**
 * Viewport only (width / scale / viewport-fit). `themeColor` and
 * `colorScheme` deliberately live in the static <head> below, not here —
 * see `themeHeadMeta` in lib/viewport.ts.
 */
export const viewport: Viewport = baseViewport;

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

  // Per-request CSP nonce for next-themes' inline no-flash script. The
  // Rust origin forwards its `Content-Security-Policy` header on the
  // proxy hop (upstream/mod.rs::forward); Next nonces its own framework
  // scripts from it automatically, but userland inline scripts —
  // next-themes' theme bootstrap is our only one — need the nonce
  // passed explicitly. A hash allowlist can't work here: the script
  // body serializes `defaultTheme`, which varies per user cookie.
  // Absent header (e.g. hitting :3000 directly) ⇒ no nonce attr, which
  // is fine — there's no CSP being enforced on that path either.
  const csp = (await headers()).get("content-security-policy");
  const nonce = /'nonce-([A-Za-z0-9+/_=-]+)'/.exec(csp ?? "")?.[1];

  // Seed the query cache with `me` so the root-level `useMe` listeners
  // (GlobalHotkeys / GlobalShortcutsSheet) don't refetch it client-side on
  // hydration (audit G7). Gated on the session cookie so anonymous pages
  // (sign-in / 404) pay no extra round-trip; `getMe()` is `cache()`-deduped
  // so an authenticated route-group layout shares this fetch instead of
  // making a second one. A stale cookie just 401s → no seed (and the group
  // layout redirects).
  const me = jar.get(SESSION_COOKIE) ? await getMe().catch(() => null) : null;

  const head = themeHeadMeta(theme);

  return (
    <html
      lang={locale}
      className="h-full"
      data-theme={dataTheme}
      data-accent={accent}
      data-density={density}
      suppressHydrationWarning
    >
      {/* Static, navigation-stable tags — see the note on `metadata`
          above. iOS snapshots the apple-* ones at Add-to-Home-Screen time:
          remove and re-add the icon after changing them.
          `black` (opaque status bar): since iOS/iPadOS 26.1 the OS reserves
          an opaque bar for home-screen apps regardless, so this keeps the
          layout identical on every OS version. `apple-mobile-web-app-capable`
          is the legacy (pre-16.4) standalone opt-in and what makes
          `navigator.standalone` true. */}
      <head>
        <meta name="apple-mobile-web-app-capable" content="yes" />
        <meta name="mobile-web-app-capable" content="yes" />
        <meta name="apple-mobile-web-app-title" content="Folio" />
        <meta name="apple-mobile-web-app-status-bar-style" content="black" />
        <meta name="color-scheme" content={head.colorScheme} />
        {head.themeColor.map((t) => (
          <meta
            key={t.media ?? "all"}
            name="theme-color"
            media={t.media}
            content={t.color}
          />
        ))}
      </head>
      <body className="bg-background text-foreground min-h-full antialiased">
        <ThemeProvider defaultTheme={theme} nonce={nonce}>
          <NextIntlClientProvider messages={messages}>
            <QueryProvider key={me?.id ?? "anonymous"} userId={me?.id}>
              {me ? <HydrateAuthCache me={me} /> : null}
              <SearchModalProvider>
                <ScanResultListener />
                <GlobalHotkeys />
                <ServiceWorkerLoader />
                <SafeAreaProbe />
                <VisualViewportSync />
                <InstallEvents />
                <GlobalShortcutsSheet>{children}</GlobalShortcutsSheet>
              </SearchModalProvider>
            </QueryProvider>
          </NextIntlClientProvider>
          <Toaster />
        </ThemeProvider>
      </body>
    </html>
  );
}
