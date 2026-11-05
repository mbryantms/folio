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
 * `state` values (see `computePullLayout` for how the indicator and
 * the page content share that distance).
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
 * - Stays on under `prefers-reduced-motion: reduce`: the movement is
 *   direct manipulation (it tracks the finger 1:1), so it is not the
 *   kind of unrequested motion the preference guards against. The
 *   indicator component drops its decorative motion (bob, spring
 *   overshoot, fan) instead.
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

/**
 * Pure layout for the indicator + page content at a given pull
 * distance. Exposed for unit testing.
 *
 * - The page content translates down by the full `distance`, opening a
 *   gap at the top of the scroll surface. The indicator lives inside
 *   that gap (its wrapper is clipped to `gap`), so it never draws over
 *   content.
 * - While the gap is shorter than the indicator, the indicator rides
 *   the gap's bottom edge (sliding in from above). Once the gap is
 *   taller, it stays centred in it. Both branches meet at
 *   `distance === indicatorHeight`, so the motion is continuous.
 * - `progress` (0..1, reaching 1 at the threshold) drives the fan-out
 *   of the covers, the opacity and the scale-in.
 */
export interface PullLayout {
  /** Pixels the page content is translated down. */
  contentOffset: number;
  /** Height of the clipped gap the indicator sits in. */
  gap: number;
  /** Indicator top within the gap (negative = partly above it). */
  indicatorY: number;
  /** 0 at rest, 1 at (and past) the threshold. */
  progress: number;
}

export function computePullLayout(
  distance: number,
  threshold: number,
  indicatorHeight: number,
): PullLayout {
  const d = Math.max(0, distance);
  const progress = threshold > 0 ? Math.min(1, d / threshold) : 1;
  const indicatorY =
    d >= indicatorHeight ? (d - indicatorHeight) / 2 : d - indicatorHeight;
  return { contentOffset: d, gap: d, indicatorY, progress };
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
  // Mirrors `state` for the touch listeners. A fast flick can deliver
  // `touchend` before React re-renders with the last `touchmove`'s
  // state, so the listeners must not rely on the render closure.
  const stateRef = useRef<PullState>("idle");
  const update = useCallback((next: PullState, d: number) => {
    stateRef.current = next;
    setState(next);
    setDistance(d);
  }, []);
  const onRefreshRef = useRef(onRefresh);
  useEffect(() => {
    onRefreshRef.current = onRefresh;
  }, [onRefresh]);

  // Stable callback the effect can read without re-binding listeners
  // every render. The state setter chain is the only place this
  // hook touches React state.
  const finishRefresh = useCallback(() => {
    update("idle", 0);
    startYRef.current = null;
  }, [update]);

  useEffect(() => {
    if (typeof window === "undefined" || typeof document === "undefined") {
      return;
    }
    if (!enabled) return;
    if (!isStandaloneDisplay()) return;

    const onTouchStart = (e: TouchEvent) => {
      if (stateRef.current === "refreshing") return;
      if (window.scrollY > 0) return;
      if (e.touches.length !== 1) return;
      startYRef.current = e.touches[0]?.clientY ?? null;
    };

    const onTouchMove = (e: TouchEvent) => {
      const startY = startYRef.current;
      if (startY === null) return;
      if (stateRef.current === "refreshing") return;
      if (window.scrollY > 0) {
        startYRef.current = null;
        update("idle", 0);
        return;
      }
      const currentY = e.touches[0]?.clientY ?? startY;
      const delta = currentY - startY;
      if (delta <= 0) {
        update("idle", 0);
        return;
      }
      const d = computePullDistance(delta, threshold, maxDistance);
      update(d >= threshold ? "armed" : "pulling", d);
    };

    const onTouchEnd = () => {
      if (startYRef.current === null) return;
      const armed = stateRef.current === "armed";
      startYRef.current = null;
      if (!armed) {
        update("idle", 0);
        return;
      }
      // Hold the content open at the threshold while refreshing.
      update("refreshing", threshold);

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
      if (stateRef.current !== "refreshing") update("idle", 0);
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
  }, [enabled, finishRefresh, maxDistance, threshold, update]);

  return { state, distance, armed: state === "armed" };
}
