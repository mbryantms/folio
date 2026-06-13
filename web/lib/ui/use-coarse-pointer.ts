"use client";

import { useEffect, useSyncExternalStore } from "react";
import { toast } from "sonner";

/**
 * Audit B16: on coarse-pointer (touch) devices there's no hover, so the
 * hover-revealed cover affordances (kebab, quick-read overlay) never
 * surface and the long-press sheet that replaces them advertises
 * nothing. These helpers detect coarse pointers so the kebab can render
 * persistently and a one-time hint can teach the gesture.
 */

const COARSE_QUERY = "(pointer: coarse)";

function subscribe(onChange: () => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mql = window.matchMedia(COARSE_QUERY);
  mql.addEventListener("change", onChange);
  return () => mql.removeEventListener("change", onChange);
}

function getSnapshot(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia(COARSE_QUERY).matches;
}

/**
 * `true` when the primary pointer is coarse (touch / stylus). SSR-safe:
 * resolves to `false` on the server and during the first client paint,
 * then settles after hydration — `useSyncExternalStore` is the idiomatic
 * subscription so there's no set-state-in-effect churn.
 */
export function useCoarsePointer(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, () => false);
}

/** localStorage flag — the hint shows at most once per browser, ever. */
const HINT_STORAGE_KEY = "folio.touchActionsHintSeen";
/** In-session guard so multiple card surfaces can't double-fire it. */
let hintFiredThisSession = false;

/**
 * Fire a one-time "tap ⋯ for actions" hint on coarse-pointer devices
 * (audit B16). Call from card-bearing surfaces (library grid, search,
 * bookmarks). No-ops on hover-capable devices and after the first show.
 */
export function useCoarsePointerActionsHint(): void {
  const coarse = useCoarsePointer();
  useEffect(() => {
    if (!coarse || hintFiredThisSession) return;
    let seen = false;
    try {
      seen = localStorage.getItem(HINT_STORAGE_KEY) === "1";
    } catch {
      // Private-mode / blocked storage — skip rather than nag every load.
      return;
    }
    if (seen) return;
    hintFiredThisSession = true;
    try {
      localStorage.setItem(HINT_STORAGE_KEY, "1");
    } catch {
      // best-effort
    }
    toast.info("Tip: tap the ⋯ on any cover for quick actions.", {
      duration: 6000,
    });
  }, [coarse]);
}
