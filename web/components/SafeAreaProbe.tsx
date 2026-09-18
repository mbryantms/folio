"use client";

import { useEffect } from "react";
import { safeTopOverride } from "@/lib/safe-area";
import { isStandaloneDisplay } from "@/lib/use-pull-to-refresh";

/**
 * Pins `--safe-top` to 0 on `<html>` once the OS is seen to reserve the
 * status bar (iOS / iPadOS 26.1+ home-screen apps). Renders nothing.
 * Rationale + the measurement live in `lib/safe-area.ts`.
 *
 * The pin is STICKY for the session. Whether the OS reserves the bar is an
 * OS policy that does not change while the app runs, but the geometry we
 * measure it from does go stale: on iPadOS 26.1 the reader route (its own
 * viewport meta + fixed chrome) produced a resize whose `innerHeight`
 * momentarily read as full-screen, the first version of this probe took
 * that at face value and dropped the pin, and nothing ever fired again to
 * restore it — the double padding came back in the reader and stayed
 * until the app was force-quit (the same stale-viewport family as
 * `use-swipe.ts`). So: a later measurement may ADD the pin, never remove
 * it. Element fullscreen hides the status bar, where 0 is right anyway.
 * A non-standalone tab never pins.
 */
export function SafeAreaProbe() {
  useEffect(() => {
    const root = document.documentElement;
    let pinned = false;
    let confirmation: ReturnType<typeof setTimeout> | undefined;
    let previous = "";
    const apply = () => {
      if (pinned) return;
      const viewport = window.visualViewport;
      if (
        viewport &&
        (viewport.scale !== 1 || window.innerHeight - viewport.height > 100)
      )
        return;
      if (
        document.activeElement?.matches(
          "input, textarea, [contenteditable=true]",
        )
      )
        return;
      const override = safeTopOverride(
        {
          innerWidth: window.innerWidth,
          innerHeight: window.innerHeight,
          screenWidth: window.screen.width,
          screenHeight: window.screen.height,
        },
        isStandaloneDisplay(),
      );
      const signature = `${window.innerWidth}:${window.innerHeight}:${override}`;
      if (override !== null && previous !== signature) {
        previous = signature;
        clearTimeout(confirmation);
        confirmation = setTimeout(apply, 100);
        return;
      }
      previous = signature;
      if (override !== null) {
        root.style.setProperty("--safe-top", override);
        pinned = true;
      }
    };
    apply();
    // Re-measure on anything that can hand us fresh geometry; each is a
    // chance to pin, never to unpin.
    window.addEventListener("resize", apply);
    window.addEventListener("orientationchange", apply);
    window.addEventListener("pageshow", apply);
    document.addEventListener("visibilitychange", apply);
    return () => {
      clearTimeout(confirmation);
      window.removeEventListener("resize", apply);
      window.removeEventListener("orientationchange", apply);
      window.removeEventListener("pageshow", apply);
      document.removeEventListener("visibilitychange", apply);
      root.style.removeProperty("--safe-top");
    };
  }, []);
  return null;
}
