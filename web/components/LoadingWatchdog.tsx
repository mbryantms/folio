"use client";

import { useEffect } from "react";

/**
 * Last-resort recovery for a stalled route transition.
 *
 * Mounted inside `loading.tsx` boundaries, so it's alive exactly while a
 * segment's RSC is pending. If that pending state outlives a generous
 * threshold, the navigation is almost certainly stuck (a proxy/upstream or
 * RSC-stream stall that the server-side `apiGet` timeout can't catch) — the
 * App Router has no hard-navigation fallback, so it would otherwise spin
 * forever until the user force-quits. We recover with a full document reload
 * of the current URL (the destination, since the App Router updates the URL
 * optimistically on navigation start).
 *
 * A sessionStorage guard prevents a reload loop: if we already hard-reloaded
 * this exact URL within the last minute, we let the stall surface rather than
 * reload endlessly (the destination itself is failing, not the navigation).
 */
const RELOAD_GUARD_KEY = "folio.loading-watchdog.last-reload";
const RELOAD_GUARD_WINDOW_MS = 60_000;

export function LoadingWatchdog({ timeoutMs = 15_000 }: { timeoutMs?: number }) {
  useEffect(() => {
    const timer = setTimeout(() => {
      const url = window.location.href;
      const now = Date.now();
      try {
        const raw = sessionStorage.getItem(RELOAD_GUARD_KEY);
        if (raw) {
          const prev = JSON.parse(raw) as { url: string; at: number };
          if (prev.url === url && now - prev.at < RELOAD_GUARD_WINDOW_MS) {
            return; // already tried recently — avoid a reload loop
          }
        }
        sessionStorage.setItem(
          RELOAD_GUARD_KEY,
          JSON.stringify({ url, at: now }),
        );
      } catch {
        // sessionStorage unavailable (private mode, etc.) — reload anyway.
      }
      window.location.reload();
    }, timeoutMs);
    return () => clearTimeout(timer);
  }, [timeoutMs]);

  return null;
}
