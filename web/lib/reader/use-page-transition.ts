"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import type { Direction } from "./detect";

/**
 * Reader page-turn animation (v0.3.44 slide; v0.3.45 fade).
 *
 * Drives a brief retain-old-page + animation each time the
 * current page changes. The caller consumes:
 *
 *   - `prevPage`: page index to render as an absolute-positioned
 *     overlay during the transition window, OR `null` for "no
 *     transition in flight, just render the current page."
 *   - `exitAnimClass`: CSS class to apply to the outgoing overlay
 *     (slides off / fades out depending on mode). `null` when
 *     mode is `off`.
 *   - `enterAnimClass`: CSS class for the incoming page wrapper's
 *     one-shot entrance animation.
 *
 * **Mode behavior:**
 *
 *   - `slide` (v0.3.44): outgoing slides off the side opposite to
 *     the turn direction; incoming slides in from the matching
 *     edge. Direction-aware via the `direction` argument.
 *
 *   - `fade` (v0.3.45): outgoing fades to opacity 0; incoming
 *     fades from 0. Direction-independent. Shortest, lowest-impact
 *     option.
 *
 *   - `off`: hook is a no-op; returns all-null. The renderer
 *     instant-swaps as usual.
 *
 *   A `curl` mode was prototyped in v0.3.45 development as a CSS
 *   rotateY effect but discarded — the rotation read as a flip
 *   card, not a corner-peel page-turn. A real corner-peel curl is
 *   queued for a later release backed by `react-pageflip` /
 *   `page-flip` (the CSS-only approach can't replicate corner-drag
 *   physics on a single image element).
 *
 * **View-mode interactions:**
 *
 *   Webtoon callers should not call this hook (continuous scroll
 *   has no per-page transition surface). Single-page renderers
 *   use both classes (overlay + new wrapper). Double-page
 *   renderers can use the enter class only — the outgoing overlay
 *   is harder to wire across variable-fit panes.
 *
 *   `prefers-reduced-motion` (OS-level): hook bypasses entirely
 *   and returns all-null. CSS keyframes also no-op via @media so
 *   any leak would still be inert.
 */
export type PageAnimationMode = "off" | "slide" | "fade";

export type PageTransitionResult = {
  prevPage: number | null;
  exitAnimClass: string | null;
  enterAnimClass: string | null;
};

/** Per-mode duration in milliseconds. The hook unmounts the
 *  outgoing overlay after this elapses; values match the keyframe
 *  durations in `styles/globals.css`. Pick the longest if multiple
 *  classes share a wrapper. */
const TRANSITION_MS: Record<PageAnimationMode, number> = {
  off: 0,
  slide: 280,
  fade: 220,
};

export function usePageTransition({
  currentPage,
  direction,
  mode,
}: {
  currentPage: number;
  direction: Direction;
  /** `null` falls back to `'slide'` (the built-in default). */
  mode: PageAnimationMode | null;
}): PageTransitionResult {
  const [prevPage, setPrevPage] = useState<number | null>(null);
  const [exitAnimClass, setExitAnimClass] = useState<string | null>(null);
  const [enterAnimClass, setEnterAnimClass] = useState<string | null>(null);
  const lastPageRef = useRef<number>(currentPage);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reduceMotion = useSyncExternalStore(
    subscribeReducedMotion,
    getReducedMotionSnapshot,
    getReducedMotionServerSnapshot,
  );

  // Resolve the user's preference into a concrete mode. `null` and
  // `'off'` both short-circuit (off is the explicit user opt-out;
  // null is "no preference" which falls through to the built-in
  // default of slide).
  const resolvedMode: PageAnimationMode = mode ?? "slide";

  useEffect(() => {
    const prev = lastPageRef.current;
    lastPageRef.current = currentPage;

    if (resolvedMode === "off" || reduceMotion) {
      // Keep the ref in sync but never emit a transition.
      return;
    }
    if (prev === currentPage) return;

    const forward = currentPage > prev;
    const slideLeft = direction === "ltr" ? forward : !forward;
    const classes = classesForMode(resolvedMode, slideLeft);

    setPrevPage(prev);
    setExitAnimClass(classes.exit);
    setEnterAnimClass(classes.enter);

    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      setPrevPage(null);
      setExitAnimClass(null);
      setEnterAnimClass(null);
    }, TRANSITION_MS[resolvedMode]);

    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [currentPage, direction, resolvedMode, reduceMotion]);

  return { prevPage, exitAnimClass, enterAnimClass };
}

function classesForMode(
  mode: PageAnimationMode,
  slideLeft: boolean,
): { exit: string | null; enter: string | null } {
  switch (mode) {
    case "off":
      return { exit: null, enter: null };
    case "slide":
      return slideLeft
        ? {
            exit: "page-slide-out-to-left",
            enter: "page-slide-in-from-right",
          }
        : {
            exit: "page-slide-out-to-right",
            enter: "page-slide-in-from-left",
          };
    case "fade":
      return { exit: "page-fade-out", enter: "page-fade-in" };
  }
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
