import type { Metadata, Viewport } from "next";
import { themedViewport } from "@/lib/viewport";
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
  // Apple-specific PWA tags. Next 16's `capable: true` emits only the
  // standardised `<meta name="mobile-web-app-capable">`; the legacy
  // `apple-mobile-web-app-capable` meta (the pre-16.4 iOS opt-in to
  // standalone launch, and what makes `navigator.standalone` true for
  // `usePullToRefresh` there) is added explicitly via `other` below.
  // iOS 16.4+ also honours the manifest's `display: standalone`.
  // Status bar style: `black` (opaque) rather than `black-translucent`.
  // Since iOS / iPadOS 26.1 the OS reserves an opaque status bar for
  // home-screen apps regardless, so translucency no longer buys the
  // edge-to-edge layout it used to; declaring the opaque bar makes the
  // layout identical on every OS version instead of depending on which
  // one the icon was installed from. `SafeAreaProbe` handles the runtime
  // side (collapsing `--safe-top` when the OS already holds the space).
  // iOS snapshots this meta at Add-to-Home-Screen time: remove and re-add
  // the icon after deploying a change here.
  appleWebApp: {
    capable: true,
    title: "Folio",
    statusBarStyle: "black",
  },
  other: { "apple-mobile-web-app-capable": "yes" },
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
 * Viewport shape + rationale live in `web/lib/viewport.ts` (shared
 * with the reader route's per-page override). This is a function
 * rather than a static export because `themeColor` / `colorScheme`
 * must follow the user's cookie-driven theme, not the device's
 * `prefers-color-scheme` — otherwise a dark-themed app on a
 * light-mode iPad declares itself white and iPadOS paints a white
 * status-bar backing over dark content in standalone mode. The
 * route is already dynamic (RootLayout reads the same cookie jar),
 * so this adds no rendering cost.
 */
export async function generateViewport(): Promise<Viewport> {
  const jar = await cookies();
  const themeCookie = jar.get(THEME_COOKIE)?.value;
  return themedViewport(isTheme(themeCookie) ? themeCookie : "dark");
}

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
