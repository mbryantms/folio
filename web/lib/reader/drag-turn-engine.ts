import type { RefObject } from "react";

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

export type DragTurnOptions = {
  enabled: boolean;
  canNext: boolean;
  canPrev: boolean;
  direction: Direction;
  onNext: () => void;
  onPrev: () => void;
};

export type DragTurnEngine = {
  onTurnDrag: (dx: number) => void;
  onTurnDragEnd: (dx: number, canceled: boolean) => boolean;
  /** The page changed: end any drag still in progress. */
  positionChanged: () => void;
  /** The caller switched drag-to-turn off. */
  disable: () => void;
  consumeSkip: () => boolean;
  dispose: () => void;
};

/**
 * The motion half of drag-to-turn (`use-drag-turn.ts`), loaded on demand
 * so it stays out of the reader's first-load JS: the hook fetches this
 * module after the first paint, and until it has arrived a swipe is a
 * plain threshold turn.
 *
 * It moves `trackRef` (a wrapper holding the current page plus, while
 * dragging, the neighbouring pages either side) 1:1 with the pointer,
 * then on release either
 *
 *  - turns: the track keeps the release speed out to a full width, then
 *    `onNext` / `onPrev` fire and the track snaps back to 0 when the hook
 *    reports the new position (`positionChanged`, from the layout effect
 *    of the commit that renders the new page), so the page that slid in
 *    is the page that's now current, without a flash;
 *  - springs back (slow / short drag, or rubber-banded at an end);
 *  - or, for a forward drag on the last page, springs back and calls
 *    `onNext` so the end-of-issue card opens as it does for any "next".
 */
export function createDragTurnEngine({
  getOptions,
  trackRef,
  setDragging,
}: {
  /** The caller's latest options (read at event time). */
  getOptions: () => DragTurnOptions;
  trackRef: RefObject<HTMLElement | null>;
  setDragging: (dragging: boolean) => void;
}): DragTurnEngine {
  let phase: Phase = "idle";
  let samples: DragSample[] = [];
  let offset = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let skipAt: number | null = null;

  const clearTimer = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };

  const apply = (x: number, ms = 0, ease = SPRING_EASE) => {
    offset = x;
    const el = trackRef.current;
    if (!el) return;
    el.style.transition = ms > 0 ? `transform ${ms}ms ${ease}` : "none";
    el.style.transform = x === 0 && ms === 0 ? "" : `translate3d(${x}px,0,0)`;
  };

  const width = () => {
    const el = trackRef.current;
    return el?.parentElement?.clientWidth || el?.clientWidth || 0;
  };

  const reset = () => {
    clearTimer();
    phase = "idle";
    samples = [];
    apply(0);
    setDragging(false);
  };

  const springBack = () => {
    clearTimer();
    if (offset === 0) {
      reset();
      return;
    }
    phase = "spring";
    apply(0, SPRING_MS, SPRING_EASE);
    timer = setTimeout(reset, SPRING_MS + 20);
  };

  const onTurnDrag = (dx: number) => {
    const o = getOptions();
    if (!o.enabled) return;
    // Mid-turn: the page is already on its way; let it land.
    if (phase === "turning" || phase === "commit") return;
    if (phase !== "drag") {
      if (dx === 0) return;
      clearTimer();
      phase = "drag";
      samples = [];
      setDragging(true);
    }
    samples.push({ x: dx, t: performance.now() });
    if (samples.length > MAX_SAMPLES) samples.shift();
    const intent = dragIntent(dx, o.direction);
    const blocked = intent === "next" ? !o.canNext : !o.canPrev;
    apply(dragOffset(dx, width(), blocked));
  };

  /** Returns true when the release was this engine's to handle (a drag
   *  it was following); false lets the caller fall back to a plain swipe. */
  const onTurnDragEnd = (dx: number, canceled: boolean): boolean => {
    if (phase === "turning" || phase === "commit") return true;
    if (phase !== "drag") return false;
    if (canceled) {
      springBack();
      return true;
    }
    const o = getOptions();
    const w = width();
    const velocity = releaseVelocity(samples, performance.now());
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
    const ms = turnDurationMs(target - offset, velocity, w);
    phase = "turning";
    apply(target, ms, TURN_EASE);
    clearTimer();
    timer = setTimeout(() => {
      phase = "commit";
      skipAt = performance.now();
      const cur = getOptions();
      if (outcome === "next") cur.onNext();
      else cur.onPrev();
      // `positionChanged` normally lands first; if the store refused the
      // turn the position never moves — come back.
      clearTimer();
      timer = setTimeout(() => {
        if (phase === "commit") springBack();
      }, COMMIT_TIMEOUT_MS);
    }, ms);
    return true;
  };

  return {
    onTurnDrag,
    onTurnDragEnd,
    positionChanged: () => {
      if (phase !== "idle") reset();
    },
    disable: () => {
      if (phase !== "idle") reset();
    },
    consumeSkip: () => {
      const at = skipAt;
      skipAt = null;
      return at !== null && performance.now() - at < SKIP_TTL_MS;
    },
    dispose: clearTimer,
  };
}
