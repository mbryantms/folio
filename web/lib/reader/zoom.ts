/**
 * Pure zoom/pan math for the single-page reader (audit C9). Kept
 * side-effect-free so it's unit-testable in the node-env harness; the
 * gesture wiring + CSS transform live in `Reader.tsx` / the gesture hook.
 */

/** Discrete zoom ladder for the `+`/`-` keybinds. Double-tap toggles
 *  between `1` and `DOUBLE_TAP_ZOOM` independently of this ladder. */
export const ZOOM_STEPS = [1, 1.5, 2, 3] as const;
export const MIN_ZOOM = ZOOM_STEPS[0];
export const MAX_ZOOM = ZOOM_STEPS[ZOOM_STEPS.length - 1];
export const DOUBLE_TAP_ZOOM = 2;
/** Double-tap recognition window + max travel between the two taps. */
export const DOUBLE_TAP_MS = 300;
export const DOUBLE_TAP_DIST = 24;

/** Next zoom level walking the ladder in/out, clamped to its bounds. */
export function nextZoomStep(current: number, dir: "in" | "out"): number {
  if (dir === "in") {
    for (const step of ZOOM_STEPS) if (step > current + 1e-3) return step;
    return MAX_ZOOM;
  }
  for (let i = ZOOM_STEPS.length - 1; i >= 0; i--) {
    if (ZOOM_STEPS[i]! < current - 1e-3) return ZOOM_STEPS[i]!;
  }
  return MIN_ZOOM;
}

/**
 * Clamp a pan offset (px) so a `scale`d page can't be dragged past its
 * own edges into empty space. At scale `s` the page is `s×` the
 * container, so each axis can travel at most `(s-1) * dimension / 2`
 * from center before an edge would pull inside the viewport.
 */
export function clampPan(
  offset: { x: number; y: number },
  scale: number,
  bounds: { w: number; h: number },
): { x: number; y: number } {
  if (scale <= 1) return { x: 0, y: 0 };
  const maxX = ((scale - 1) * bounds.w) / 2;
  const maxY = ((scale - 1) * bounds.h) / 2;
  return {
    x: Math.max(-maxX, Math.min(maxX, offset.x)),
    y: Math.max(-maxY, Math.min(maxY, offset.y)),
  };
}

/** Transform-origin (as `%`) at a tap/click point within a rect, so a
 *  double-tap zooms in around where the user tapped. Clamped to [0,100]. */
export function zoomOriginPercent(
  tapX: number,
  tapY: number,
  rect: { w: number; h: number },
): { x: number; y: number } {
  const pct = (v: number, dim: number) =>
    dim <= 0 ? 50 : Math.max(0, Math.min(100, (v / dim) * 100));
  return { x: pct(tapX, rect.w), y: pct(tapY, rect.h) };
}

export type TapSample = { t: number; x: number; y: number };

/** Whether `next` completes a double-tap with `prev` (within `ms` and
 *  `dist`). `prev` null (no prior tap) is never a double. */
export function isDoubleTap(
  prev: TapSample | null,
  next: TapSample,
  ms: number = DOUBLE_TAP_MS,
  dist: number = DOUBLE_TAP_DIST,
): boolean {
  if (!prev) return false;
  if (next.t - prev.t > ms) return false;
  return Math.hypot(next.x - prev.x, next.y - prev.y) <= dist;
}
