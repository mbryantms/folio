"use client";

import { useEffect, useSyncExternalStore } from "react";
import { toast } from "sonner";

/**
 * Audit B16: on coarse-pointer (touch) devices there's no hover, so the
 * hover-revealed cover affordances (kebab, quick-read overlay) never
 * surface. The cover **long-press sheet** is the touch way in; the kebab
 * is hidden there entirely (it otherwise just sits on top of the cover and
 * obscures the art). These helpers detect the relevant device class so the
 * kebab can hide and a one-time hint can teach the long-press gesture.
 */

const COARSE_QUERY = "(pointer: coarse)";
const TOUCH_QUERY = "(hover: none) and (pointer: coarse)";

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

function subscribeTouch(onChange: () => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mql = window.matchMedia(TOUCH_QUERY);
  mql.addEventListener("change", onChange);
  return () => mql.removeEventListener("change", onChange);
}

function getTouchSnapshot(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia(TOUCH_QUERY).matches;
}

/**
 * `true` only on hover-incapable coarse-pointer devices (phones, touch-only
 * tablets — iOS / iPadOS without a trackpad). This is the exact device class
 * where the cover long-press sheet is the way in, so the kebab hides here and
 * the long-press sheet activates here. Distinct from {@link useCoarsePointer}
 * (`(pointer: coarse)` alone), which also matches coarse+hover hybrids — using
 * the narrower query keeps the "kebab hidden" and "sheet active" conditions
 * identical, so no device falls through both.
 */
export function useTouchDevice(): boolean {
  return useSyncExternalStore(subscribeTouch, getTouchSnapshot, () => false);
}

/** localStorage flag — the hint shows at most once per browser, ever. The
 *  `.v2` suffix re-shows it once: the original copy taught "tap the ⋯",
 *  which no longer exists on touch now that the kebab is hidden. */
const HINT_STORAGE_KEY = "folio.touchActionsHintSeen.v2";
/** In-session guard so multiple card surfaces can't double-fire it. */
let hintFiredThisSession = false;

/**
 * Fire a one-time "long-press a cover for actions" hint on touch devices
 * (audit B16). Call from card-bearing surfaces (library grid, search,
 * bookmarks). No-ops on hover-capable devices and after the first show.
 */
export function useCoarsePointerActionsHint(): void {
  const touch = useTouchDevice();
  useEffect(() => {
    if (!touch || hintFiredThisSession) return;
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
    toast.info("Tip: long-press any cover for quick actions.", {
      duration: 6000,
    });
  }, [touch]);
}
