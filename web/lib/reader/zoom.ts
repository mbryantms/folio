/**
 * Pure zoom/pan math for the single- and double-page reader (audit C9,
 * WP-4.2). Kept side-effect-free so it's unit-testable in the node-env
 * harness; the gesture wiring + CSS transform live in `Reader.tsx`, the
 * drag hook (`use-swipe.ts`) and the wheel/pinch hook (`use-wheel-zoom.ts`).
 */

/** Discrete zoom ladder for the `+`/`-` keybinds. Double-tap toggles
 *  between `1` and `DOUBLE_TAP_ZOOM` independently of this ladder. */
export const ZOOM_STEPS = [1, 1.5, 2, 3] as const;
export const MIN_ZOOM = ZOOM_STEPS[0];
export const MAX_ZOOM = ZOOM_STEPS[ZOOM_STEPS.length - 1]!;
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
 * Clamp a pan offset (px) so the rendered content can't be dragged past
 * its own edges into empty space. Generalized over `content` (the
 * rendered size: container×scale when zoomed, or the natural image size
 * when a fit=height/original page overflows the viewport at 1×) vs
 * `container` (the visible box): each axis can travel at most
 * `(content - container) / 2` from center. Axes where content ≤
 * container are pinned to 0.
 */
export function clampPan(
  offset: { x: number; y: number },
  content: { w: number; h: number },
  container: { w: number; h: number },
): { x: number; y: number } {
  const maxX = Math.max(0, (content.w - container.w) / 2);
  const maxY = Math.max(0, (content.h - container.h) / 2);
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

// ---- Transform-zoom state (single + double view) --------------------------

/** The reader's transform-zoom state. `offset` is a screen-px translate
 *  applied before `scale`; `origin` is the CSS transform-origin in % of
 *  the zoom surface's (untransformed) layout box. */
export type ZoomState = {
  scale: number;
  offset: { x: number; y: number };
  origin: { x: number; y: number };
};

export const ZOOM_IDENTITY: ZoomState = {
  scale: 1,
  offset: { x: 0, y: 0 },
  origin: { x: 50, y: 50 },
};

/** Below this the zoom snaps back to exactly 1× (re-centered), so a
 *  wheel zoom-out never strands the page at 1.003× in pan mode. */
const SNAP_TO_ONE = 1.01;

/** Clamp a continuous (wheel / pinch) scale into the zoom range. */
export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) return MIN_ZOOM;
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, scale));
}

/** Per-event cap on the normalized wheel delta, so a single fast
 *  mouse-wheel notch or a flung trackpad can't jump the whole range. */
const WHEEL_DELTA_CAP = 25;
/** Exponential wheel→scale sensitivity: one capped notch ≈ ×1.28. */
const WHEEL_ZOOM_SENSITIVITY = 0.01;
const WHEEL_LINE_PX = 16;
const WHEEL_PAGE_PX = 800;

/**
 * Multiplicative scale factor for one `wheel` event carrying the zoom
 * intent (`ctrlKey` — which is also how Chromium / Firefox / Edge report
 * a trackpad pinch on desktop). `deltaMode` is normalized to pixels
 * (0 = px, 1 = lines, 2 = pages). Scrolling down / pinching in
 * (positive delta) zooms out; the reverse zooms in.
 */
export function wheelZoomFactor(deltaY: number, deltaMode = 0): number {
  const px =
    deltaMode === 1
      ? deltaY * WHEEL_LINE_PX
      : deltaMode === 2
        ? deltaY * WHEEL_PAGE_PX
        : deltaY;
  const capped = Math.max(-WHEEL_DELTA_CAP, Math.min(WHEEL_DELTA_CAP, px));
  return Math.exp(-capped * WHEEL_ZOOM_SENSITIVITY);
}

/**
 * Pan clamp for a *zoomed* surface (scale > 1) with an arbitrary
 * transform-origin. With `translate(T) scale(s)` about origin `O` (px),
 * the scaled box's leading edge sits at `T + O·(1 − s)`; keeping the
 * box covering the visible `container` bounds `T` to
 * `[(1 − s)(w − O), (s − 1)·O]` per axis. For a centered origin this is
 * exactly {@link clampPan}'s `±(s·w − w)/2`; for a corner or tapped
 * origin (double-tap, page-turn carry) it is the correct asymmetric
 * range instead of the centered approximation.
 */
export function clampZoomPan(
  offset: { x: number; y: number },
  scale: number,
  originPct: { x: number; y: number },
  container: { w: number; h: number },
): { x: number; y: number } {
  if (scale <= 1) return { x: 0, y: 0 };
  const axis = (t: number, dim: number, pct: number) => {
    const o = (pct / 100) * dim;
    const min = (1 - scale) * (dim - o);
    const max = (scale - 1) * o;
    return Math.max(min, Math.min(max, t));
  };
  return {
    x: axis(offset.x, container.w, originPct.x),
    y: axis(offset.y, container.h, originPct.y),
  };
}

/**
 * Zoom to `nextScale` keeping the content under `point` stationary
 * (wheel / trackpad pinch zoom-at-cursor). `point` is in px relative to
 * the zoom surface's untransformed layout box; `box` is that box's size.
 *
 * With screen position `S(p) = T + O + s·(p − O)`, holding `S(p)` at the
 * cursor while `s → s'` gives `T' = q − (s'/s)·(q − T)` where
 * `q = point − O`. The origin is left as-is so there's no jump, and the
 * result is clamped so the page can't be dragged off its own edges.
 * Scales at (or snapping to) 1× return {@link ZOOM_IDENTITY}.
 */
export function zoomAboutPoint(
  z: ZoomState,
  nextScale: number,
  point: { x: number; y: number },
  box: { w: number; h: number },
): ZoomState {
  const s2 = clampScale(nextScale);
  if (s2 < SNAP_TO_ONE) return ZOOM_IDENTITY;
  const s1 = z.scale > 0 ? z.scale : 1;
  const ox = (z.origin.x / 100) * box.w;
  const oy = (z.origin.y / 100) * box.h;
  const qx = point.x - ox;
  const qy = point.y - oy;
  const ratio = s2 / s1;
  const raw = {
    x: qx - ratio * (qx - z.offset.x),
    y: qy - ratio * (qy - z.offset.y),
  };
  return {
    scale: s2,
    origin: z.origin,
    offset: clampZoomPan(raw, s2, z.origin, box),
  };
}

/**
 * Zoom state for the next page after a page turn.
 *
 * Default (`persist` off): zoom is transient per page — back to 1×.
 * With the "keep zoom between pages" preference on, the scale carries
 * over and the view lands on the **reading-order start corner** of the
 * new page (top-left in LTR, top-right in RTL) by pinning the
 * transform-origin to that corner with no offset — no page metrics are
 * needed, and {@link clampZoomPan} keeps later pans exact for it.
 */
export function zoomAfterPageTurn(
  z: ZoomState,
  persist: boolean,
  direction: "ltr" | "rtl",
): ZoomState {
  if (!persist || z.scale <= 1) return ZOOM_IDENTITY;
  return {
    scale: z.scale,
    offset: { x: 0, y: 0 },
    origin: { x: direction === "rtl" ? 100 : 0, y: 0 },
  };
}

/** Structural equality for zoom states (skip no-op `setState`s). */
export function sameZoom(a: ZoomState, b: ZoomState): boolean {
  return (
    a.scale === b.scale &&
    a.offset.x === b.offset.x &&
    a.offset.y === b.offset.y &&
    a.origin.x === b.origin.x &&
    a.origin.y === b.origin.y
  );
}
