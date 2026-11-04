"use client";

import { useEffect, useLayoutEffect, useRef } from "react";

const MAX_ANIMATED = 60;
const DURATION_MS = 360;

type Rects = Map<string, DOMRect>;

function measure(root: HTMLElement | null): Rects {
  const out: Rects = new Map();
  if (!root) return out;
  const vh = window.innerHeight;
  for (const el of root.querySelectorAll<HTMLElement>("[data-flip-id]")) {
    const r = el.getBoundingClientRect();
    if (r.bottom < -100 || r.top > vh + 100) continue;
    const id = el.dataset.flipId;
    if (id) out.set(id, r);
  }
  return out;
}

/**
 * "Smooth card-size changes" (folio-flair-1.0, standard). When the
 * card-size slider moves, every on-screen element marked `data-flip-id`
 * glides from where it was to where it now is, instead of the grid
 * snapping to the new column count. Works across remounts (the virtualized
 * grid rebuilds its rows) because elements are matched by id.
 *
 * The old layout is read when the size-change event fires: the slider's
 * setter broadcasts `beforeEvent` in the same handler that sets state, so
 * React has not re-rendered yet. The animation runs in a layout effect
 * once `trigger` (the card size) has changed, before paint. Reduced motion
 * skips it.
 */
export function useFlipOnChange(
  rootRef: React.RefObject<HTMLElement | null>,
  trigger: unknown,
  beforeEvent: string,
  reduced: boolean,
) {
  const before = useRef<Rects | null>(null);
  const prevTrigger = useRef(trigger);

  useEffect(() => {
    if (reduced) return;
    const onBefore = () => {
      // Keep the first snapshot of a burst (slider drag): it is the layout
      // actually on screen until the next commit.
      before.current ??= measure(rootRef.current);
    };
    window.addEventListener(beforeEvent, onBefore);
    return () => window.removeEventListener(beforeEvent, onBefore);
  }, [beforeEvent, reduced, rootRef]);

  useLayoutEffect(() => {
    if (prevTrigger.current === trigger) return;
    prevTrigger.current = trigger;
    const old = before.current;
    before.current = null;
    if (!old || old.size === 0) return;
    const root = rootRef.current;
    const now = measure(root);
    let n = 0;
    for (const [id, to] of now) {
      const from = old.get(id);
      if (!from || to.width === 0 || n >= MAX_ANIMATED) continue;
      const dx = from.left - to.left;
      const dy = from.top - to.top;
      const s = from.width / to.width;
      if (Math.abs(dx) < 1 && Math.abs(dy) < 1 && Math.abs(s - 1) < 0.01)
        continue;
      const el = root?.querySelector<HTMLElement>(
        `[data-flip-id="${CSS.escape(id)}"]`,
      );
      if (!el) continue;
      n++;
      el.animate(
        [
          {
            transformOrigin: "top left",
            transform: `translate(${dx}px, ${dy}px) scale(${s})`,
          },
          { transformOrigin: "top left", transform: "none" },
        ],
        { duration: DURATION_MS, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
      );
    }
  }, [trigger, rootRef]);
}
