import { useEffect, useRef, type RefObject } from "react";
import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import { wheelZoomFactor } from "@/lib/reader/zoom";

/**
 * Whether a `wheel` event carries zoom intent. Ctrl+wheel is the desktop
 * zoom chord, and Chromium / Firefox / Edge report a **trackpad pinch**
 * as a synthetic `wheel` with `ctrlKey: true` — so one check covers both.
 * (macOS Safari reports trackpad pinch as `gesture*` events instead; see
 * the hook below.)
 */
export function isZoomWheel(e: { ctrlKey: boolean }): boolean {
  return e.ctrlKey;
}

type WheelZoomOpts = {
  target: RefObject<HTMLElement | null>;
  /** False in webtoon / marker-drawing: the event falls through to the
   *  browser untouched. */
  enabled: boolean;
  /** Current scale, read at event time. */
  getScale: () => number;
  /** Zoom to `scale`, anchored at the client-space point. */
  onZoomTo: (scale: number, clientX: number, clientY: number) => void;
};

/** WebKit's non-standard trackpad-pinch event (macOS Safari). */
type GestureEventLike = Event & {
  scale: number;
  clientX: number;
  clientY: number;
};

/**
 * Ctrl+wheel / trackpad-pinch zoom for the reader (audit UX-3, WP-4.2).
 *
 * Without this, a desktop pinch became *browser* page zoom, which scales
 * the fixed chrome and page strip along with the art. The listener is
 * non-passive so it can `preventDefault` the browser zoom and drive the
 * reader's own transform zoom instead, anchored at the cursor.
 *
 * Safari: trackpad pinch arrives as `gesturestart` / `gesturechange`
 * (cumulative `scale` since gesture start) rather than ctrl+wheel. Those
 * are handled only on fine-pointer devices — on iOS the same events fire
 * for a two-finger *touch* pinch, which stays native pinch-zoom there.
 */
export function useWheelZoom(opts: WheelZoomOpts): void {
  const optsRef = useRef(opts);
  useEffect(() => {
    optsRef.current = opts;
  });

  const { target } = opts;
  useEffect(() => {
    const el = target.current;
    if (!el) return;

    const onWheel = (e: WheelEvent) => {
      const { enabled, getScale, onZoomTo } = optsRef.current;
      if (!enabled || !isZoomWheel(e)) return;
      e.preventDefault();
      onZoomTo(
        getScale() * wheelZoomFactor(e.deltaY, e.deltaMode),
        e.clientX,
        e.clientY,
      );
    };
    el.addEventListener("wheel", onWheel, { passive: false });

    const handleGestures = !primaryPointerIsCoarse();
    let base = 1;
    const onGestureStart = (e: Event) => {
      if (!optsRef.current.enabled) return;
      e.preventDefault();
      base = optsRef.current.getScale();
    };
    const onGestureChange = (e: Event) => {
      const { enabled, onZoomTo } = optsRef.current;
      if (!enabled) return;
      e.preventDefault();
      const g = e as GestureEventLike;
      if (typeof g.scale !== "number") return;
      onZoomTo(base * g.scale, g.clientX, g.clientY);
    };
    const onGestureEnd = (e: Event) => {
      if (optsRef.current.enabled) e.preventDefault();
    };
    if (handleGestures) {
      el.addEventListener("gesturestart", onGestureStart, { passive: false });
      el.addEventListener("gesturechange", onGestureChange, {
        passive: false,
      });
      el.addEventListener("gestureend", onGestureEnd, { passive: false });
    }
    return () => {
      el.removeEventListener("wheel", onWheel);
      if (handleGestures) {
        el.removeEventListener("gesturestart", onGestureStart);
        el.removeEventListener("gesturechange", onGestureChange);
        el.removeEventListener("gestureend", onGestureEnd);
      }
    };
  }, [target]);
}
