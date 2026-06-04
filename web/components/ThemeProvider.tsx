"use client";

import { ThemeProvider as NextThemesProvider } from "next-themes";

/**
 * Themes are managed by next-themes via the `data-theme` attribute on
 * `<html>`. We disable transitions on theme change so the swap is instant
 * (rendering radius/colour at 60fps mid-animation looks bad).
 * `enableSystem` lets a stored `system` preference resolve through
 * `prefers-color-scheme`; next-themes injects its no-flash script before
 * hydration so the initial client paint agrees with the resolved OS theme.
 *
 * The `data-accent` and `data-density` attributes are set server-side from
 * cookies in the locale layout and updated client-side by the
 * `ThemeAccentSync` and `ThemeDensitySync` helpers when the user picks a
 * new value.
 */
export function ThemeProvider({
  children,
  ...props
}: React.ComponentProps<typeof NextThemesProvider>) {
  return (
    <NextThemesProvider
      attribute="data-theme"
      defaultTheme="system"
      enableSystem
      themes={["dark", "light", "amber"]}
      disableTransitionOnChange
      {...props}
    >
      {children}
    </NextThemesProvider>
  );
}
