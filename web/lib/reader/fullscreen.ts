"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * Reader-side wrapper around the Fullscreen API. Returns the live
 * fullscreen state plus a toggle.
 *
 * Always targets `document.documentElement` so html/body remain the
 * scrolling root inside fullscreen — fullscreening a div instead would
 * clip overflowing content (a fit-width page taller than the viewport
 * has no scrollable ancestor above the fullscreen element).
 *
 * Errors from `requestFullscreen` / `exitFullscreen` are intentionally
 * swallowed — the API rejects when permission is denied or the document
 * isn't allowed to enter fullscreen, and there's nothing useful to surface
 * to the user.
 */
export function useFullscreen(): { isFullscreen: boolean; toggle: () => void } {
  const isFullscreen = useSyncExternalStore(
    subscribeFullscreen,
    getFullscreenSnapshot,
    getServerSnapshot,
  );
  const toggle = useCallback(() => {
    if (typeof document === "undefined") return;
    if (document.fullscreenElement) {
      void document.exitFullscreen?.().catch(() => undefined);
    } else {
      void document.documentElement
        .requestFullscreen?.()
        .catch(() => undefined);
    }
  }, []);
  return { isFullscreen, toggle };
}

function subscribeFullscreen(cb: () => void): () => void {
  if (typeof document === "undefined") return () => undefined;
  document.addEventListener("fullscreenchange", cb);
  return () => document.removeEventListener("fullscreenchange", cb);
}

function getFullscreenSnapshot(): boolean {
  return typeof document !== "undefined" ? !!document.fullscreenElement : false;
}

function getServerSnapshot(): boolean {
  return false;
}

/**
 * Reading-progress percent for the top progress bar. Returns 100 when the
 * user is on the final page, scales linearly otherwise, clamps to [0,100].
 * Convention matches "Page X of Y" — page 1 reads as `1/Y` covered, the
 * last page as `100%`.
 */
export function readingPercent(
  currentPage: number,
  totalPages: number,
): number {
  if (totalPages <= 0) return 0;
  const pct = ((currentPage + 1) / totalPages) * 100;
  if (pct < 0) return 0;
  if (pct > 100) return 100;
  return pct;
}
