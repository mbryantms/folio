"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import type { Direction } from "./detect";

/**
 * Reader page-turn slide animation (v0.3.44).
 *
 * Drives a brief retain-old-page + slide overlay each time the
 * current page changes. The caller (Reader.tsx single + double
 * modes) consumes:
 *
 *   - `prevPage`: page index to render absolutely-positioned with
 *     the outgoing transform, OR `null` for "no transition in
 *     flight, just render the current page normally."
 *   - `exitDir`: direction the outgoing page is sliding off
 *     (`'left' | 'right'`). Caller maps these to Tailwind
 *     `transition-transform translate-x-{full|-full}` classes.
 *   - `enterAnimClass`: CSS class to apply to the incoming page
 *     wrapper for its one-shot slide-in. One of
 *     `'page-slide-in-from-right' | 'page-slide-in-from-left'`,
 *     defined in `styles/globals.css`.
 *
 * Webtoon view mode is the wrong place for per-page transitions —
 * callers should not call this hook in webtoon mode (the
 * continuous-scroll renderer is its own animation surface).
 *
 * The hook honors `prefers-reduced-motion`: it skips the retain
 * entirely (returns `null`s) so reduced-motion users get the
 * existing instant page swap.
 *
 * Setting `enabled=false` (user preference `default_page_animation
 * = 'off'`) bypasses the retain too — same behavior as
 * reduced-motion.
 */
export type PageTransitionResult = {
  prevPage: number | null;
  exitDir: "left" | "right" | null;
  enterAnimClass: string | null;
};

const TRANSITION_MS = 280;

export function usePageTransition({
  currentPage,
  direction,
  enabled,
}: {
  currentPage: number;
  direction: Direction;
  /** `true` when `default_page_animation = 'slide'` AND the current
   *  view mode is single/double (webtoon callers should pass false
   *  or skip the hook entirely). `prefers-reduced-motion` is read
   *  inside the hook itself. */
  enabled: boolean;
}): PageTransitionResult {
  const [prevPage, setPrevPage] = useState<number | null>(null);
  const [exitDir, setExitDir] = useState<"left" | "right" | null>(null);
  const [enterAnimClass, setEnterAnimClass] = useState<string | null>(null);
  const lastPageRef = useRef<number>(currentPage);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // `useSyncExternalStore` is the React-18+ pattern for subscribing
  // to external mutable values without tripping the
  // `react-hooks/set-state-in-effect` lint or hydration mismatch
  // warnings. Server snapshot is `false` (no media-query info
  // available); client snapshot reads matchMedia directly.
  const reduceMotion = useSyncExternalStore(
    subscribeReducedMotion,
    getReducedMotionSnapshot,
    getReducedMotionServerSnapshot,
  );

  useEffect(() => {
    const prev = lastPageRef.current;
    lastPageRef.current = currentPage;

    if (!enabled || reduceMotion) {
      // Bypass — keep the ref in sync but never emit a transition.
      // If the user turns the preference on mid-session OR toggles
      // off reduced-motion, the next page change will animate; the
      // current swap is instant.
      return;
    }
    if (prev === currentPage) return;

    // Direction of intent: did the user advance (next page) or go
    // back (prev page)? LTR forward + RTL backward both slide the
    // outgoing page LEFT (with the incoming page coming from the
    // RIGHT). LTR backward + RTL forward slide outgoing RIGHT and
    // incoming from the LEFT.
    const forward = currentPage > prev;
    const slideLeft = direction === "ltr" ? forward : !forward;
    setPrevPage(prev);
    setExitDir(slideLeft ? "left" : "right");
    setEnterAnimClass(
      slideLeft ? "page-slide-in-from-right" : "page-slide-in-from-left",
    );

    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      setPrevPage(null);
      setExitDir(null);
      setEnterAnimClass(null);
    }, TRANSITION_MS);

    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [currentPage, direction, enabled, reduceMotion]);

  return { prevPage, exitDir, enterAnimClass };
}

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

function subscribeReducedMotion(callback: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  const mql = window.matchMedia(REDUCED_MOTION_QUERY);
  mql.addEventListener("change", callback);
  return () => mql.removeEventListener("change", callback);
}

function getReducedMotionSnapshot(): boolean {
  if (typeof window === "undefined") return false;
  return window.matchMedia(REDUCED_MOTION_QUERY).matches;
}

function getReducedMotionServerSnapshot(): boolean {
  return false;
}
