/**
 * Touch pinch → reader transform zoom (audit N-R9, remediation C5).
 *
 * Until this module the reader handed a two-finger pinch to the browser,
 * so on a phone or tablet the *page* zoomed natively — along with the
 * fixed chrome, the page strip and the status-bar backing — and the
 * reader's own transform zoom (ctrl+wheel / trackpad on desktop) was
 * reachable on touch only through double-tap. Now a pinch over the
 * single- or double-page surface drives the same `ZoomState` the desktop
 * path drives: the content under the fingers' midpoint stays put while
 * the scale follows the finger distance, moving both fingers together
 * pans, and lifting leaves the page at the final scale (1× snaps to the
 * identity exactly like the wheel path, see `zoomAboutPoint`). Nothing
 * animates — the transform tracks the fingers 1:1 — so there is nothing
 * for `prefers-reduced-motion` to disable.
 *
 * Why touch events and not pointer events: the standalone iOS / iPadOS
 * web app can stop delivering the captured `pointermove` / `pointerup`
 * stream after an in-app navigation (see `coarse-pointer.ts`); every
 * touch-drag surface in the reader binds through touch events on
 * coarse-pointer devices for that reason, and a pinch is a touch-only
 * gesture to begin with.
 *
 * WebKit's non-standard `gesture*` events: iOS fires them for a
 * two-finger touch pinch *and* for a trackpad pinch (iPad + Magic
 * Keyboard). Cancelling them is the lever that stops iOS Safari's native
 * zoom (it ignores `user-scalable=no`). With `handleGestures` on — the
 * coarse-primary-pointer devices `use-wheel-zoom.ts` leaves alone — a
 * gesture that is *not* backed by a touch pinch (the trackpad) also
 * drives the zoom from the event's cumulative `scale`, so a trackpad
 * pinch on an iPad zooms the page like a desktop one does.
 *
 * The module is loaded lazily on touch-capable devices only
 * (`use-pinch-zoom.ts`) so it stays out of the reader's first-load chunk.
 * The math is pure and unit-tested in `tests/reader/pinch-zoom.test.ts`;
 * the DOM wiring in `tests/dom/reader-pinch-zoom.test.tsx`.
 */
import {
  clampZoomPan,
  zoomAboutPoint,
  type ZoomState,
} from "@/lib/reader/zoom";

export type PinchPoint = { x: number; y: number };

/** Midpoint + finger distance of a two-finger touch, in zoom-surface px. */
export type PinchGeometry = { mid: PinchPoint; dist: number };

/** The zoom surface's untransformed layout box in client coordinates:
 *  `left` / `top` of its (static) parent plus its own offset size. */
export type SurfaceBox = { left: number; top: number; w: number; h: number };

export function pinchGeometry(a: PinchPoint, b: PinchPoint): PinchGeometry {
  return {
    mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 },
    dist: Math.hypot(b.x - a.x, b.y - a.y),
  };
}

/**
 * Zoom state for the current finger positions of a pinch that started
 * with the page at `start` and the fingers at `from`. Cumulative from the
 * gesture start (not incremental from the previous move) so the 1×
 * snap in `zoomAboutPoint` can't swallow a slow zoom-in one tiny step at
 * a time, and so float drift never accumulates across a long pinch.
 *
 * Two parts: scale about the *starting* midpoint so the art under the
 * fingers stays under them, then follow the midpoint's travel as a pan —
 * clamped like any other pan so the page can't leave its own edges.
 * `box` is the surface's layout size; both geometries are relative to
 * its origin.
 */
export function applyPinch(
  start: ZoomState,
  from: PinchGeometry,
  to: PinchGeometry,
  box: { w: number; h: number },
): ZoomState {
  if (from.dist <= 0) return start;
  const zoomed = zoomAboutPoint(
    start,
    start.scale * (to.dist / from.dist),
    from.mid,
    box,
  );
  if (zoomed.scale <= 1) return zoomed;
  const dx = to.mid.x - from.mid.x;
  const dy = to.mid.y - from.mid.y;
  if (dx === 0 && dy === 0) return zoomed;
  return {
    ...zoomed,
    offset: clampZoomPan(
      { x: zoomed.offset.x + dx, y: zoomed.offset.y + dy },
      zoomed.scale,
      zoomed.origin,
      box,
    ),
  };
}

export type PinchZoomOpts = {
  /** False in webtoon / while drawing a marker: the touch is left alone
   *  (and `readerTouchAction` hands pinch back to the browser). */
  enabled: () => boolean;
  /** Current zoom, read when the second finger lands. */
  getZoom: () => ZoomState;
  setZoom: (z: ZoomState) => void;
  /** The live zoom surface, or null when no page view is mounted. */
  getSurface: () => SurfaceBox | null;
  /** Cancel (and, for a trackpad, drive the zoom from) WebKit `gesture*`
   *  events. On for coarse-primary-pointer devices, where
   *  `use-wheel-zoom.ts` does not read them. */
  handleGestures: boolean;
};

export type PinchZoomHandle = {
  /** True while two fingers are down, and until the next single-finger
   *  touch begins after a pinch — the drag layer (`use-swipe.ts`) reads
   *  it so the finger that outlives a pinch never turns a page or drags
   *  a stale pan. */
  isPinching: () => boolean;
  detach: () => void;
};

type TouchLike = { clientX: number; clientY: number };
type TouchEventLike = Event & { touches: ArrayLike<TouchLike> };
type GestureEventLike = Event & {
  scale: number;
  clientX: number;
  clientY: number;
};

type PinchStart = { zoom: ZoomState; geom: PinchGeometry; surface: SurfaceBox };

/** `idle` → second finger lands → `pinching` → a finger lifts →
 *  `settled` (the lift must not read as a swipe) → a fresh single-finger
 *  touch → `idle`. */
type Phase = "idle" | "pinching" | "settled";

const GESTURE_EVENTS = ["gesturestart", "gesturechange", "gestureend"];

export function attachPinchZoom(
  el: HTMLElement,
  opts: PinchZoomOpts,
): PinchZoomHandle {
  let phase: Phase = "idle";
  let start: PinchStart | null = null;
  let gestureStart: { zoom: ZoomState; surface: SurfaceBox } | null = null;

  const geometry = (touches: ArrayLike<TouchLike>, s: SurfaceBox) =>
    pinchGeometry(
      { x: touches[0]!.clientX - s.left, y: touches[0]!.clientY - s.top },
      { x: touches[1]!.clientX - s.left, y: touches[1]!.clientY - s.top },
    );

  const onTouchStart = (e: Event) => {
    const { touches } = e as TouchEventLike;
    if (touches.length === 1) {
      phase = "idle";
      start = null;
      return;
    }
    if (touches.length < 2 || !opts.enabled()) {
      start = null;
      return;
    }
    const surface = opts.getSurface();
    if (!surface) return;
    // Claim the gesture before the browser starts its own pinch-zoom.
    e.preventDefault();
    phase = "pinching";
    // The surface box is fixed for the gesture (the touch is cancelled,
    // so nothing scrolls); reading it per move would force a layout.
    start = { zoom: opts.getZoom(), geom: geometry(touches, surface), surface };
  };
  const onTouchMove = (e: Event) => {
    const { touches } = e as TouchEventLike;
    if (!start || touches.length < 2) return;
    // The first finger had already started a native `pan-y` scroll when
    // the second landed: the browser keeps that scroll and delivers
    // uncancelable moves. Zooming on top would fight it, so stand down —
    // the next pinch from a still page is claimed normally.
    if (!e.cancelable) {
      start = null;
      return;
    }
    e.preventDefault();
    const { surface } = start;
    opts.setZoom(
      applyPinch(start.zoom, start.geom, geometry(touches, surface), {
        w: surface.w,
        h: surface.h,
      }),
    );
  };
  const onTouchEnd = (e: Event) => {
    if ((e as TouchEventLike).touches.length < 2) {
      start = null;
      if (phase === "pinching") phase = "settled";
    }
  };

  // A gesture backed by a touch pinch (two fingers on the screen) is only
  // cancelled — the touch path above owns it. One that is not (an iPad
  // trackpad) is driven here from WebKit's cumulative `scale`.
  const onGestureStart = (e: Event) => {
    if (!opts.enabled()) return;
    e.preventDefault();
    if (phase === "pinching") return;
    const surface = opts.getSurface();
    gestureStart = surface ? { zoom: opts.getZoom(), surface } : null;
  };
  const onGestureChange = (e: Event) => {
    if (!opts.enabled()) return;
    e.preventDefault();
    if (phase === "pinching" || !gestureStart) return;
    const g = e as GestureEventLike;
    if (typeof g.scale !== "number") return;
    const { zoom, surface } = gestureStart;
    opts.setZoom(
      zoomAboutPoint(
        zoom,
        zoom.scale * g.scale,
        { x: g.clientX - surface.left, y: g.clientY - surface.top },
        { w: surface.w, h: surface.h },
      ),
    );
  };
  const onGestureEnd = (e: Event) => {
    if (opts.enabled()) e.preventDefault();
    gestureStart = null;
  };

  el.addEventListener("touchstart", onTouchStart, { passive: false });
  el.addEventListener("touchmove", onTouchMove, { passive: false });
  el.addEventListener("touchend", onTouchEnd, { passive: true });
  el.addEventListener("touchcancel", onTouchEnd, { passive: true });
  const gestureHandlers = [onGestureStart, onGestureChange, onGestureEnd];
  if (opts.handleGestures) {
    GESTURE_EVENTS.forEach((name, i) =>
      el.addEventListener(name, gestureHandlers[i]!, { passive: false }),
    );
  }
  return {
    isPinching: () => phase !== "idle",
    detach: () => {
      el.removeEventListener("touchstart", onTouchStart);
      el.removeEventListener("touchmove", onTouchMove);
      el.removeEventListener("touchend", onTouchEnd);
      el.removeEventListener("touchcancel", onTouchEnd);
      if (opts.handleGestures) {
        GESTURE_EVENTS.forEach((name, i) =>
          el.removeEventListener(name, gestureHandlers[i]!),
        );
      }
    },
  };
}
