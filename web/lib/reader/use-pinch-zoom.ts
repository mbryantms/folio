import { useCallback, useEffect, useRef, type RefObject } from "react";
import {
  anyPointerIsCoarse,
  primaryPointerIsCoarse,
} from "@/lib/reader/coarse-pointer";
import type { PinchZoomHandle, SurfaceBox } from "@/lib/reader/pinch-zoom";
import type { ZoomState } from "@/lib/reader/zoom";

type PinchZoomHookOpts = {
  target: RefObject<HTMLElement | null>;
  /** False in webtoon / marker-drawing: the touch is left to the browser. */
  enabled: boolean;
  getZoom: () => ZoomState;
  setZoom: (z: ZoomState) => void;
  getSurface: () => SurfaceBox | null;
};

/**
 * Touch pinch → transform zoom (audit N-R9). The handlers live in
 * `pinch-zoom.ts`, which is imported lazily and only on touch-capable
 * devices (`any-pointer: coarse`), so the reader's first-load chunk
 * carries just this shim (bundle budget: `scripts/check-bundle-size.mjs`).
 * WebKit `gesture*` events are handled there only where `useWheelZoom`
 * leaves them alone (coarse primary pointer: iPad, with or without a
 * trackpad). Options are read at event time through a ref, like
 * `useWheelZoom`. The returned `isPinching` is stable and safe to hand
 * to the drag layer before the module has loaded (it answers false
 * until then).
 */
export function usePinchZoom(opts: PinchZoomHookOpts): {
  isPinching: () => boolean;
} {
  const optsRef = useRef(opts);
  useEffect(() => {
    optsRef.current = opts;
  });
  const handleRef = useRef<PinchZoomHandle | null>(null);

  const { target } = opts;
  useEffect(() => {
    if (!anyPointerIsCoarse()) return;
    const el = target.current;
    if (!el) return;
    const handleGestures = primaryPointerIsCoarse();
    let cancelled = false;
    void import("@/lib/reader/pinch-zoom").then((m) => {
      if (cancelled) return;
      handleRef.current = m.attachPinchZoom(el, {
        enabled: () => optsRef.current.enabled,
        getZoom: () => optsRef.current.getZoom(),
        setZoom: (z) => optsRef.current.setZoom(z),
        getSurface: () => optsRef.current.getSurface(),
        handleGestures,
      });
    });
    return () => {
      cancelled = true;
      handleRef.current?.detach();
      handleRef.current = null;
    };
  }, [target]);

  const isPinching = useCallback(
    () => handleRef.current?.isPinching() ?? false,
    [],
  );
  return { isPinching };
}
