"use client";

import { useEffect } from "react";
import { safeTopOverride } from "@/lib/safe-area";
import { isStandaloneDisplay } from "@/lib/use-pull-to-refresh";

/**
 * Pins `--safe-top` to 0 on `<html>` when the OS has already reserved the
 * status bar (iOS / iPadOS 26.1+ home-screen apps), and removes the pin
 * again if the viewport turns out to run edge to edge. Renders nothing.
 * Rationale + the measurement live in `lib/safe-area.ts`.
 */
export function SafeAreaProbe() {
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      const override = safeTopOverride(
        {
          innerWidth: window.innerWidth,
          innerHeight: window.innerHeight,
          screenWidth: window.screen.width,
          screenHeight: window.screen.height,
        },
        isStandaloneDisplay(),
      );
      if (override === null) root.style.removeProperty("--safe-top");
      else root.style.setProperty("--safe-top", override);
    };
    apply();
    window.addEventListener("resize", apply);
    window.addEventListener("orientationchange", apply);
    return () => {
      window.removeEventListener("resize", apply);
      window.removeEventListener("orientationchange", apply);
      root.style.removeProperty("--safe-top");
    };
  }, []);
  return null;
}
