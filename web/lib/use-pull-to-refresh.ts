"use client";

import { useCallback, useEffect, useRef, useState } from "react";

/**
 * iOS Safari's native pull-to-refresh is disabled whenever a page
 * is launched in standalone mode (PWA from the Home Screen). There
 * is no setting to re-enable it — applications running outside the
 * Safari chrome have to implement pull-to-refresh themselves.
 *
 * `usePullToRefresh` provides that gesture with the same general
 * feel as the system one: a finger drag down at the top of the
 * scroll surface, springy resistance past the threshold, and a
 * visual indicator that's driven from the returned `distance` /
 * `state` values.
 *
 * Activation rules:
 * - Only enables itself when the viewport is in standalone display
 *   mode (PWA installed to Home Screen). In a regular browser tab
 *   the system already provides pull-to-refresh — competing with it
 *   would feel broken.
 * - Only initiates a pull when the scroll position is at the top.
 *   A pull started mid-page is treated as a normal scroll.
 * - Only initiates on touch input. Mouse / trackpad users can use
 *   the browser's reload button or `Cmd+R`; pull-to-refresh on a
 *   trackpad would conflict with two-finger scrolling.
 * - Disables itself when `prefers-reduced-motion: reduce` is set,
 *   because the gesture is movement-heavy by definition.
 */

export type PullState = "idle" | "pulling" | "armed" | "refreshing";

export interface UsePullToRefreshOpts {
  /**
   * Called when the user pulls past the threshold and releases. The
   * hook stays in `refreshing` state until the returned promise
   * settles (or 6 seconds elapse as a safety cap).
   */
  onRefresh: () => Promise<unknown> | void;
  /**
   * Manual override — pass `false` to disable the gesture (for
   * example, on a route that owns its own touch surface like the
   * reader). Defaults to `true`.
   */
  enabled?: boolean;
  /**
   * Distance in CSS pixels past which a release triggers the
   * refresh. Default 80. Below the threshold the indicator shrinks
   * back to zero on release without firing.
   */
  threshold?: number;
  /**
   * Hard cap on visible pull distance. The drag may exceed this in
   * raw input, but the indicator stops growing here. Default 140.
   */
  maxDistance?: number;
}

const DEFAULT_THRESHOLD = 80;
const DEFAULT_MAX = 140;
const REFRESH_TIMEOUT_MS = 6_000;

/**
 * Pure resistance curve. Exposed for unit testing. The curve is
 * linear up to the threshold, then asymptotic so the indicator
 * still moves past it (giving the "armed" affordance) but never
 * runs away off the bottom of the screen.
 */
export function computePullDistance(
  rawPullPx: number,
  threshold: number,
  maxDistance: number,
): number {
  if (rawPullPx <= 0) return 0;
  if (rawPullPx <= threshold) return rawPullPx;
  const overshoot = rawPullPx - threshold;
  const room = maxDistance - threshold;
  // Asymptotic approach: distance = threshold + room * (1 - 1/(1 + overshoot/room))
  // At overshoot = room → adds room/2.  At overshoot = ∞ → adds room.
  return threshold + room * (1 - 1 / (1 + overshoot / room));
}

/**
 * Standalone-mode detection. Covers both the modern
 * `display-mode: standalone` media query (every platform that
 * honours the web app manifest's `display` field) and Safari's
 * legacy `navigator.standalone` boolean (still the most reliable
 * signal on iOS, where the manifest is partially honored).
 */
export function isStandaloneDisplay(): boolean {
  if (typeof window === "undefined") return false;
  if (window.matchMedia("(display-mode: standalone)").matches) return true;
  const nav = window.navigator as Navigator & { standalone?: boolean };
  return nav.standalone === true;
}

export interface UsePullToRefreshReturn {
  state: PullState;
  distance: number;
  /** Becomes `true` once the user has pulled past the threshold. */
  armed: boolean;
}

export function usePullToRefresh(
  opts: UsePullToRefreshOpts,
): UsePullToRefreshReturn {
  const {
    onRefresh,
    enabled = true,
    threshold = DEFAULT_THRESHOLD,
    maxDistance = DEFAULT_MAX,
  } = opts;

  const [state, setState] = useState<PullState>("idle");
  const [distance, setDistance] = useState(0);

  const startYRef = useRef<number | null>(null);
  const onRefreshRef = useRef(onRefresh);
  useEffect(() => {
    onRefreshRef.current = onRefresh;
  }, [onRefresh]);

  // Stable callback the effect can read without re-binding listeners
  // every render. The state setter chain is the only place this
  // hook touches React state.
  const finishRefresh = useCallback(() => {
    setState("idle");
    setDistance(0);
    startYRef.current = null;
  }, []);

  useEffect(() => {
    if (typeof window === "undefined" || typeof document === "undefined") {
      return;
    }
    if (!enabled) return;
    if (!isStandaloneDisplay()) return;

    const reducedMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    if (reducedMotion) return;

    const onTouchStart = (e: TouchEvent) => {
      if (state === "refreshing") return;
      if (window.scrollY > 0) return;
      if (e.touches.length !== 1) return;
      startYRef.current = e.touches[0]?.clientY ?? null;
    };

    const onTouchMove = (e: TouchEvent) => {
      const startY = startYRef.current;
      if (startY === null) return;
      if (state === "refreshing") return;
      if (window.scrollY > 0) {
        startYRef.current = null;
        setDistance(0);
        setState("idle");
        return;
      }
      const currentY = e.touches[0]?.clientY ?? startY;
      const delta = currentY - startY;
      if (delta <= 0) {
        setDistance(0);
        setState("idle");
        return;
      }
      const d = computePullDistance(delta, threshold, maxDistance);
      setDistance(d);
      setState(d >= threshold ? "armed" : "pulling");
    };

    const onTouchEnd = () => {
      const armed = state === "armed";
      startYRef.current = null;
      if (!armed) {
        setDistance(0);
        setState("idle");
        return;
      }
      setState("refreshing");
      setDistance(threshold);

      let settled = false;
      const settle = () => {
        if (settled) return;
        settled = true;
        finishRefresh();
      };

      const timeout = window.setTimeout(settle, REFRESH_TIMEOUT_MS);
      const result = onRefreshRef.current();
      if (result && typeof (result as Promise<unknown>).then === "function") {
        (result as Promise<unknown>).finally(() => {
          window.clearTimeout(timeout);
          settle();
        });
      } else {
        window.clearTimeout(timeout);
        settle();
      }
    };

    const onTouchCancel = () => {
      startYRef.current = null;
      if (state !== "refreshing") {
        setDistance(0);
        setState("idle");
      }
    };

    window.addEventListener("touchstart", onTouchStart, { passive: true });
    window.addEventListener("touchmove", onTouchMove, { passive: true });
    window.addEventListener("touchend", onTouchEnd, { passive: true });
    window.addEventListener("touchcancel", onTouchCancel, { passive: true });

    return () => {
      window.removeEventListener("touchstart", onTouchStart);
      window.removeEventListener("touchmove", onTouchMove);
      window.removeEventListener("touchend", onTouchEnd);
      window.removeEventListener("touchcancel", onTouchCancel);
    };
  }, [enabled, finishRefresh, maxDistance, state, threshold]);

  return { state, distance, armed: state === "armed" };
}
