import type { Metadata } from "next";
import { NextIntlClientProvider } from "next-intl";
import { getLocale, getMessages } from "next-intl/server";
import { cookies } from "next/headers";
import { GlobalHotkeys } from "@/components/GlobalHotkeys";
import { QueryProvider } from "@/components/QueryProvider";
import { ScanResultListener } from "@/components/ScanResultListener";
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
  title: "Comic Reader",
  description: "Self-hostable comic reading platform",
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
              <ScanResultListener />
              <GlobalHotkeys />
              {children}
            </QueryProvider>
          </NextIntlClientProvider>
          <Toaster />
        </ThemeProvider>
      </body>
    </html>
  );
}
