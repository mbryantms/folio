"use client";
import { useEffect } from "react";
import { usePathname } from "next/navigation";
import { useTheme } from "next-themes";
import { THEME_COLORS } from "@/lib/pwa/theme-colors";

/** Keep browser chrome in step with immediate client theme changes. Dark
 * reader navigation retains the library color to avoid WebKit's latch bug. */
export function ThemeChromeSync() {
  const { resolvedTheme } = useTheme();
  const pathname = usePathname();
  useEffect(() => {
    const theme =
      resolvedTheme === "light" || resolvedTheme === "amber"
        ? resolvedTheme
        : "dark";
    const reader = pathname?.startsWith("/read/");
    const color = reader && theme !== "dark" ? "#000000" : THEME_COLORS[theme];
    const scheme = reader || theme === "dark" ? "dark" : "light";
    const apply = () => {
      document.documentElement.style.colorScheme = scheme;
      document
        .querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')
        .forEach((meta) => {
          if (meta.content !== color) meta.content = color;
        });
      const meta = document.querySelector<HTMLMetaElement>(
        'meta[name="color-scheme"]',
      );
      if (meta && meta.content !== scheme) meta.content = scheme;
    };
    apply();
    // Next can stream route metadata after the effect has run.
    const observer = new MutationObserver(apply);
    observer.observe(document.head, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["content"],
    });
    return () => {
      observer.disconnect();
      document.documentElement.style.removeProperty("color-scheme");
    };
  }, [resolvedTheme, pathname]);
  return null;
}
