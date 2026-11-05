"use client";

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react";

import type { Direction } from "./detect";
import {
  decideDragTurn,
  dragIntent,
  dragOffset,
  releaseVelocity,
  turnDurationMs,
  type DragSample,
} from "./drag-turn";

/** Spring-back duration and curve (slight ease-out, no overshoot). */
const SPRING_MS = 260;
const SPRING_EASE = "cubic-bezier(0.25, 1, 0.5, 1)";
/** Turn settle curve: starts at the release speed and decelerates. */
const TURN_EASE = "cubic-bezier(0.2, 0.7, 0.3, 1)";
/** A committed turn whose page change never lands (the store refused
 *  it) springs back after this long. */
const COMMIT_TIMEOUT_MS = 250;
/** The "skip the page-turn animation" flag set for a drag turn expires
 *  after this long, so a turn the transition hook never looked at (mode
 *  off) can't swallow a later one. */
const SKIP_TTL_MS = 600;
const MAX_SAMPLES = 12;

type Phase = "idle" | "drag" | "turning" | "commit" | "spring";

/**
 * Drag-to-turn with momentum for the paged reader (single / double view).
 *
 * The gesture itself stays in `useReaderGestures` (`use-swipe.ts`) — the
 * reader's single drag-claim layer — which forwards the live horizontal
 * movement to `onTurnDrag` and the release to `onTurnDragEnd`. This hook
 * owns the motion: it moves `trackRef` (a wrapper holding the current
 * page plus, while `dragging`, the neighbouring pages either side) 1:1
 * with the pointer, then on release either
 *
 *  - turns: the track keeps the release speed out to a full width, then
 *    `onNext` / `onPrev` fire and the track snaps back to 0 in the layout
 *    effect of the same commit that renders the new page — so the page
 *    that slid in is the page that's now current, without a flash;
 *  - springs back (slow / short drag, or rubber-banded at an end);
 *  - or, for a forward drag on the last page, springs back and calls
 *    `onNext` so the end-of-issue card opens as it does for any "next".
 *
 * Drag turns skip the regular page-turn animation (`consumeSkip`, read by
 * `usePageTransition`): the drag already animated them. The page curl
 * never arms for a drag.
 *
 * `enabled` is the caller's gate (paged mode, no marker in flight, motion
 * allowed). Under reduced motion the caller leaves this off and swipes
 * fall back to a plain threshold turn.
 */
export function useDragTurn({
  enabled,
  position,
  canNext,
  canPrev,
  direction,
  onNext,
  onPrev,
  trackRef,
}: {
  enabled: boolean;
  /** Page index (single) or spread-group index (double). */
  position: number;
  canNext: boolean;
  canPrev: boolean;
  direction: Direction;
  onNext: () => void;
  onPrev: () => void;
  trackRef: RefObject<HTMLElement | null>;
}) {
  const [dragging, setDragging] = useState(false);
  const phaseRef = useRef<Phase>("idle");
  const samplesRef = useRef<DragSample[]>([]);
  const offsetRef = useRef(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const skipAtRef = useRef<number | null>(null);
  const optsRef = useRef({
    enabled,
    canNext,
    canPrev,
    direction,
    onNext,
    onPrev,
  });
  useLayoutEffect(() => {
    optsRef.current = { enabled, canNext, canPrev, direction, onNext, onPrev };
  });

  const clearTimer = () => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = null;
  };

  const apply = useCallback(
    (x: number, ms = 0, ease = SPRING_EASE) => {
      offsetRef.current = x;
      const el = trackRef.current;
      if (!el) return;
      el.style.transition = ms > 0 ? `transform ${ms}ms ${ease}` : "none";
      el.style.transform = x === 0 && ms === 0 ? "" : `translate3d(${x}px,0,0)`;
    },
    [trackRef],
  );

  const width = useCallback(() => {
    const el = trackRef.current;
    return el?.parentElement?.clientWidth || el?.clientWidth || 0;
  }, [trackRef]);

  const reset = useCallback(() => {
    clearTimer();
    phaseRef.current = "idle";
    samplesRef.current = [];
    apply(0);
    setDragging(false);
  }, [apply]);

  const springBack = useCallback(() => {
    clearTimer();
    if (offsetRef.current === 0) {
      reset();
      return;
    }
    phaseRef.current = "spring";
    apply(0, SPRING_MS, SPRING_EASE);
    timerRef.current = setTimeout(reset, SPRING_MS + 20);
  }, [apply, reset]);

  const onTurnDrag = useCallback(
    (dx: number) => {
      const o = optsRef.current;
      if (!o.enabled) return;
      const phase = phaseRef.current;
      // Mid-turn: the page is already on its way; let it land.
      if (phase === "turning" || phase === "commit") return;
      if (phase !== "drag") {
        if (dx === 0) return;
        clearTimer();
        phaseRef.current = "drag";
        samplesRef.current = [];
        setDragging(true);
      }
      const samples = samplesRef.current;
      samples.push({ x: dx, t: performance.now() });
      if (samples.length > MAX_SAMPLES) samples.shift();
      const intent = dragIntent(dx, o.direction);
      const blocked = intent === "next" ? !o.canNext : !o.canPrev;
      apply(dragOffset(dx, width(), blocked));
    },
    [apply, width],
  );

  /** Returns true when the release was this hook's to handle (a drag it
   *  was following); false lets the caller fall back to a plain swipe. */
  const onTurnDragEnd = useCallback(
    (dx: number, canceled: boolean): boolean => {
      const phase = phaseRef.current;
      if (phase === "turning" || phase === "commit") return true;
      if (phase !== "drag") return false;
      if (canceled) {
        springBack();
        return true;
      }
      const o = optsRef.current;
      const w = width();
      const velocity = releaseVelocity(samplesRef.current, performance.now());
      const outcome = decideDragTurn({
        dx,
        velocity,
        width: w,
        direction: o.direction,
        canNext: o.canNext,
        canPrev: o.canPrev,
      });
      if (outcome === "back") {
        springBack();
        return true;
      }
      if (outcome === "end") {
        springBack();
        o.onNext();
        return true;
      }
      const target = dx < 0 ? -w : w;
      const ms = turnDurationMs(target - offsetRef.current, velocity, w);
      phaseRef.current = "turning";
      apply(target, ms, TURN_EASE);
      clearTimer();
      timerRef.current = setTimeout(() => {
        phaseRef.current = "commit";
        skipAtRef.current = performance.now();
        const cur = optsRef.current;
        if (outcome === "next") cur.onNext();
        else cur.onPrev();
        // The layout effect below normally lands first; if the store
        // refused the turn the position never moves — come back.
        clearTimer();
        timerRef.current = setTimeout(() => {
          if (phaseRef.current === "commit") springBack();
        }, COMMIT_TIMEOUT_MS);
      }, ms);
      return true;
    },
    [apply, springBack, width],
  );

  // The page changed. After a committed drag turn this lands in the same
  // commit that renders the new page: snap the track home before paint.
  // Any other page change (key, tap, strip) ends a drag in progress too.
  const lastPositionRef = useRef(position);
  useLayoutEffect(() => {
    if (lastPositionRef.current === position) return;
    lastPositionRef.current = position;
    if (phaseRef.current !== "idle") reset();
  }, [position, reset]);

  useEffect(() => {
    if (!enabled && phaseRef.current !== "idle") reset();
  }, [enabled, reset]);

  useEffect(() => () => clearTimer(), []);

  const consumeSkip = useCallback(() => {
    const at = skipAtRef.current;
    skipAtRef.current = null;
    return at !== null && performance.now() - at < SKIP_TTL_MS;
  }, []);

  return { dragging, onTurnDrag, onTurnDragEnd, consumeSkip };
}
