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
import type { DragTurnEngine, DragTurnOptions } from "./drag-turn-engine";
import { loadDragTurnEngine } from "./drag-turn-loader";

/** Run `fn` once the browser is idle after the first paint (or soon). */
function onIdle(fn: () => void): () => void {
  if (typeof window.requestIdleCallback === "function") {
    const id = window.requestIdleCallback(fn, { timeout: 2000 });
    return () => window.cancelIdleCallback(id);
  }
  const t = setTimeout(fn, 300);
  return () => clearTimeout(t);
}

/**
 * Drag-to-turn with momentum for the paged reader (single / double view).
 *
 * The gesture itself stays in `useReaderGestures` (`use-swipe.ts`) — the
 * reader's single drag-claim layer — which forwards the live horizontal
 * movement to `onTurnDrag` and the release to `onTurnDragEnd`. The motion
 * (follow the pointer, turn with the release speed or spring back) lives
 * in `drag-turn-engine.ts`, which is not part of the reader's first-load
 * JS: this hook fetches it when the browser is first idle after the page
 * has painted, or on the first drag if that comes sooner, and calls `warm`
 * at the same moment so the caller can fetch its peek view too.
 *
 * Until the engine has arrived — and for good if its chunk fails to load
 * (offline with a cold cache, a deploy that removed it) — `onTurnDrag` does
 * nothing and `onTurnDragEnd` returns false, so a swipe falls back to the
 * plain threshold turn; keys and tap zones never depend on it. A failed
 * load is retried on a later drag.
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
  warm,
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
  /** Called once, when the engine is first fetched (e.g. to preload the
   *  neighbouring-page view shown while dragging). */
  warm?: () => void;
}) {
  const [dragging, setDragging] = useState(false);
  const engineRef = useRef<DragTurnEngine | null>(null);
  const loadingRef = useRef(false);
  const aliveRef = useRef(true);
  const optsRef = useRef<DragTurnOptions & { warm?: () => void }>({
    enabled,
    canNext,
    canPrev,
    direction,
    onNext,
    onPrev,
    warm,
  });
  useLayoutEffect(() => {
    optsRef.current = {
      enabled,
      canNext,
      canPrev,
      direction,
      onNext,
      onPrev,
      warm,
    };
  });

  const load = useCallback(() => {
    if (engineRef.current || loadingRef.current) return;
    loadingRef.current = true;
    optsRef.current.warm?.();
    loadDragTurnEngine().then(
      (mod) => {
        loadingRef.current = false;
        if (!aliveRef.current || engineRef.current) return;
        engineRef.current = mod.createDragTurnEngine({
          getOptions: () => optsRef.current,
          trackRef,
          setDragging,
        });
      },
      () => {
        // Swipes keep turning plainly; the next drag tries again.
        loadingRef.current = false;
      },
    );
  }, [trackRef]);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      engineRef.current?.dispose();
    };
  }, []);

  // Fetch the engine once the first page is up and the browser is idle.
  useEffect(() => {
    if (!enabled) {
      engineRef.current?.disable();
      return;
    }
    if (engineRef.current) return;
    return onIdle(load);
  }, [enabled, load]);

  const onTurnDrag = useCallback(
    (dx: number) => {
      const engine = engineRef.current;
      if (engine) {
        engine.onTurnDrag(dx);
        return;
      }
      if (optsRef.current.enabled) load();
    },
    [load],
  );

  /** Returns true when the release was the engine's to handle (a drag it
   *  was following); false lets the caller fall back to a plain swipe. */
  const onTurnDragEnd = useCallback(
    (dx: number, canceled: boolean): boolean =>
      engineRef.current?.onTurnDragEnd(dx, canceled) ?? false,
    [],
  );

  // The page changed. After a committed drag turn this lands in the same
  // commit that renders the new page: snap the track home before paint.
  // Any other page change (key, tap, strip) ends a drag in progress too.
  const lastPositionRef = useRef(position);
  useLayoutEffect(() => {
    if (lastPositionRef.current === position) return;
    lastPositionRef.current = position;
    engineRef.current?.positionChanged();
  }, [position]);

  const consumeSkip = useCallback(
    () => engineRef.current?.consumeSkip() ?? false,
    [],
  );

  return { dragging, onTurnDrag, onTurnDragEnd, consumeSkip };
}
