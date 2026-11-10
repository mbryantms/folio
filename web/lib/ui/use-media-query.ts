"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * `true` while `query` matches. SSR-safe: `false` on the server and during
 * hydration, then settles on the client (`useSyncExternalStore`, so there is
 * no set-state-in-effect churn). Layout that must be right before paint
 * belongs in CSS; use this only for behaviour that follows the layout.
 */
export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      if (typeof window === "undefined" || !window.matchMedia) return () => {};
      const mql = window.matchMedia(query);
      mql.addEventListener("change", onChange);
      return () => mql.removeEventListener("change", onChange);
    },
    [query],
  );
  const getSnapshot = useCallback(() => {
    if (typeof window === "undefined" || !window.matchMedia) return false;
    return window.matchMedia(query).matches;
  }, [query]);
  return useSyncExternalStore(subscribe, getSnapshot, () => false);
}
