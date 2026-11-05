import type { Direction } from "./detect";

/**
 * Pure physics behind drag-to-turn in the paged reader
 * (`use-drag-turn.ts`): the page follows the finger 1:1, a release past
 * `DISTANCE_RATIO` of the page width or a flick faster than
 * `FLICK_VELOCITY` turns the page, anything else springs back, and a drag
 * past the first / last page meets rubber-band resistance.
 */

/** Fraction of the viewport width a slow drag must cover to turn. */
export const DISTANCE_RATIO = 0.3;
/** Release speed (px/ms) that turns the page regardless of distance. */
export const FLICK_VELOCITY = 0.45;
/** A flick still has to travel this far — a jittery tap isn't a turn. */
export const FLICK_MIN_PX = 16;
/** Velocity is measured over the samples in this trailing window. */
export const VELOCITY_WINDOW_MS = 100;

const MIN_TURN_MS = 90;
const MAX_TURN_MS = 320;
/** Settle speed for a distance-triggered (slow) release: a full width in
 *  this many ms. */
const BASE_TURN_MS = 300;

/**
 * Rubber-band offset for a drag of `dx` px that has nowhere to go (first
 * page dragged back, last page dragged forward). Asymptotic: it never
 * passes half the width however far the pointer travels, and starts at
 * roughly half the pointer's speed.
 */
export function rubberBand(dx: number, width: number): number {
  if (width <= 0 || dx === 0) return 0;
  const w = width;
  return Math.sign(dx) * (1 - 1 / ((Math.abs(dx) * 0.55) / w + 1)) * w * 0.5;
}

/** Turn intent of a horizontal drag: dragging the page left reads
 *  forward in LTR, backward in RTL. */
export function dragIntent(dx: number, direction: Direction): "next" | "prev" {
  const forward = direction === "rtl" ? dx > 0 : dx < 0;
  return forward ? "next" : "prev";
}

/** On-screen offset (px) for a live drag of `dx`. */
export function dragOffset(
  dx: number,
  width: number,
  blocked: boolean,
): number {
  if (blocked) return rubberBand(dx, width);
  return Math.max(-width, Math.min(width, dx));
}

export type DragSample = { x: number; t: number };

/** Signed release velocity (px/ms) over the trailing window of samples.
 *  Zero when the pointer had come to rest before the release. */
export function releaseVelocity(
  samples: readonly DragSample[],
  now: number,
  windowMs: number = VELOCITY_WINDOW_MS,
): number {
  if (samples.length < 2) return 0;
  const last = samples[samples.length - 1]!;
  // Pointer parked before letting go: no flick.
  if (now - last.t > windowMs) return 0;
  let first = last;
  for (let i = samples.length - 1; i >= 0; i -= 1) {
    const s = samples[i]!;
    if (now - s.t > windowMs) break;
    first = s;
  }
  const dt = last.t - first.t;
  if (dt <= 0) return 0;
  return (last.x - first.x) / Math.max(8, dt);
}

export type DragTurnOutcome = "next" | "prev" | "back" | "end";

/**
 * What a released drag does: turn (`next` / `prev`), spring `back`, or
 * — a forward drag on the last page — `end` (spring back and open the
 * end-of-issue card, like any other "next" on the last page).
 */
export function decideDragTurn({
  dx,
  velocity,
  width,
  direction,
  canNext,
  canPrev,
}: {
  dx: number;
  /** Signed px/ms, same axis as `dx`. */
  velocity: number;
  width: number;
  direction: Direction;
  canNext: boolean;
  canPrev: boolean;
}): DragTurnOutcome {
  if (dx === 0 || width <= 0) return "back";
  const far = Math.abs(dx) > DISTANCE_RATIO * width;
  const flick =
    Math.abs(velocity) > FLICK_VELOCITY &&
    Math.sign(velocity) === Math.sign(dx) &&
    Math.abs(dx) >= FLICK_MIN_PX;
  // A drag carried far and then thrown back the other way is a change
  // of mind, not a turn.
  const reversed =
    Math.abs(velocity) > FLICK_VELOCITY &&
    Math.sign(velocity) !== Math.sign(dx);
  if (!(far || flick) || reversed) return "back";
  const intent = dragIntent(dx, direction);
  if (intent === "next") return canNext ? "next" : "end";
  return canPrev ? "prev" : "back";
}

/** Duration (ms) of the settle animation after a turning release: the
 *  page keeps the release speed (never slower than a full width in
 *  ~300ms), bounded so a flick still reads as motion. */
export function turnDurationMs(
  remainingPx: number,
  velocity: number,
  width: number,
): number {
  const base = width > 0 ? width / BASE_TURN_MS : 1;
  const speed = Math.max(Math.abs(velocity), base);
  const ms = Math.abs(remainingPx) / speed;
  return Math.round(Math.min(MAX_TURN_MS, Math.max(MIN_TURN_MS, ms)));
}
