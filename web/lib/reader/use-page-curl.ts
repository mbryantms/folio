"use client";

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useSyncExternalStore,
} from "react";

import type { Direction } from "./detect";
import { readerPageImages } from "./page-dom";
// Type-only: the curl code itself is loaded on demand (below), so it
// never weighs on the reader's first-load JS and costs nothing when off.
import type * as PageCurlModule from "./page-curl";
import type { PageSnap } from "./page-curl";

type CurlModule = typeof PageCurlModule;

const FINE_POINTER = "(hover: hover) and (pointer: fine)";

function subscribeFine(cb: () => void) {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mq = window.matchMedia(FINE_POINTER);
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}
const fineSnapshot = () =>
  typeof window !== "undefined" && !!window.matchMedia?.(FINE_POINTER).matches;

/** An armed turn older than this is stale (the arm didn't lead to a page
 *  change — e.g. "next" on the last page opened the end card). */
const ARM_TTL_MS = 600;

type Armed = {
  forward: boolean;
  position: number;
  from: PageSnap[];
  at: number;
};

/** Snapshot the page images on screen, or null when any of them isn't
 *  fully visible (overflowing / scrolled / zoomed pages don't curl). */
function snapshotPages(): PageSnap[] | null {
  const imgs = readerPageImages();
  if (imgs.length === 0) return null;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const snaps: PageSnap[] = [];
  for (const img of imgs) {
    const r = img.getBoundingClientRect();
    if (r.width < 8 || r.height < 8) return null;
    if (r.left < -1 || r.top < -1 || r.right > vw + 1 || r.bottom > vh + 1) {
      return null;
    }
    snaps.push({
      rect: { left: r.left, top: r.top, width: r.width, height: r.height },
      src: img.currentSrc || img.src,
      srcset: img.currentSrc ? undefined : img.srcset || undefined,
      sizes: img.currentSrc ? undefined : img.sizes || undefined,
      filter: getComputedStyle(img).filter,
    });
  }
  return snaps;
}

/**
 * Page-curl turns for the paged reader (effect `page_curl`).
 *
 * The caller calls `arm(forward)` right before a *non-swipe* turn (key,
 * click, tap zone) — while the outgoing page is still in the DOM, so its
 * rect and decoded source can be snapshotted. When `position` (page index
 * in single view, spread-group index in double view) then moves by exactly
 * one step in that direction, a layout effect measures the incoming page
 * and plays the curl over it before paint. Swipes never arm, so they keep
 * the regular page-turn animation; so do jumps (page strip, `g g`, …),
 * which never move by exactly one step after an arm.
 *
 * `skipTransition()` tells `usePageTransition` that the turn it is about to
 * animate is being curled instead. It is read from that hook's passive
 * effect, which runs after this hook's layout effect in the same commit.
 *
 * Only active on fine-pointer devices (`(hover: hover) and (pointer:
 * fine)`); `enabled` carries the user's switch (reduced motion already
 * folded in by `useVisualEffect`) and the paged-mode / zoom gates.
 */
export function usePageCurl({
  enabled,
  position,
  direction,
}: {
  enabled: boolean;
  position: number;
  direction: Direction;
}) {
  const fine = useSyncExternalStore(subscribeFine, fineSnapshot, () => false);
  const active = enabled && fine;

  const activeRef = useRef(active);
  const positionRef = useRef(position);
  const armedRef = useRef<Armed | null>(null);
  const curlingRef = useRef(false);
  const stopRef = useRef<(() => void) | null>(null);
  const modRef = useRef<CurlModule | null>(null);

  // Fetch the curl module once the effect is live. Until it resolves,
  // turns fall back to the regular animation.
  useEffect(() => {
    if (!active || modRef.current) return;
    let alive = true;
    void import("./page-curl").then((m) => {
      if (alive) modRef.current = m;
    });
    return () => {
      alive = false;
    };
  }, [active]);

  useLayoutEffect(() => {
    activeRef.current = active;
    if (!active) {
      armedRef.current = null;
      stopRef.current?.();
      stopRef.current = null;
    }
  }, [active]);

  useLayoutEffect(() => {
    const prev = positionRef.current;
    positionRef.current = position;
    curlingRef.current = false;
    if (prev === position) return;
    // Any turn ends a curl still in flight.
    stopRef.current?.();
    stopRef.current = null;
    const armed = armedRef.current;
    armedRef.current = null;
    const mod = modRef.current;
    if (!armed || !activeRef.current || !mod) return;
    if (performance.now() - armed.at > ARM_TTL_MS) return;
    if (armed.position !== prev) return;
    if (position - prev !== (armed.forward ? 1 : -1)) return;
    const to = snapshotPages();
    if (!to) return;
    const plan = mod.planCurl({
      direction,
      forward: armed.forward,
      from: armed.from,
      to,
    });
    if (!plan) return;
    stopRef.current = mod.runPageCurl(plan);
    curlingRef.current = true;
  }, [position, direction]);

  useEffect(
    () => () => {
      stopRef.current?.();
      stopRef.current = null;
    },
    [],
  );

  const arm = useCallback((forward: boolean) => {
    if (!activeRef.current || !modRef.current) return;
    const from = snapshotPages();
    armedRef.current = from
      ? {
          forward,
          position: positionRef.current,
          from,
          at: performance.now(),
        }
      : null;
  }, []);

  const skipTransition = useCallback(() => curlingRef.current, []);

  return { arm, skipTransition };
}
