"use client";

import * as React from "react";

/** A toolbar row is "one row" while its content box is no taller than
 *  one touch-floored control (44px) plus slack. Two wrapped rows of the
 *  36px desktop controls measure 80px, so 60px separates the cases on
 *  both fine and coarse pointers. */
export const ONE_ROW_MAX_PX = 60;

/**
 * Desktop counterpart of the phone bar's `sticky top-(--topbar-h)`:
 * pin a wrapping toolbar under the topbar, but only while it fits on
 * one row. A toolbar that has folded onto two or three lines (iPad with
 * the sidebar open, a narrow window) would pin 100px+ of chrome, so it
 * stays in flow instead.
 *
 * `stuck` reports whether the row is currently held by its sticky
 * position (the page has scrolled past where it would sit in flow), so
 * the band's border / backdrop can appear only then and the page looks
 * unchanged at rest. It is measured against `sentinelRef`, a zero-height
 * element rendered immediately before the toolbar: in flow the two are
 * adjacent; once the toolbar sticks, the sentinel keeps scrolling and a
 * gap opens. That keeps the probe independent of the topbar's height.
 */
export function useStickyToolbar({ enabled = true }: { enabled?: boolean }) {
  const ref = React.useRef<HTMLDivElement | null>(null);
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  const [oneRow, setOneRow] = React.useState(false);
  const [stuck, setStuck] = React.useState(false);

  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const h = entries[0]?.contentRect.height;
      if (typeof h === "number") setOneRow(h > 0 && h <= ONE_ROW_MAX_PX);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const sticky = enabled && oneRow;

  // No synchronous reset when `sticky` drops: the return value masks
  // the stale flag with `sticky && stuck`, and the next engagement
  // re-measures via rAF before anything renders on it.
  React.useEffect(() => {
    if (!sticky) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      const s = sentinelRef.current;
      const t = ref.current;
      if (!s || !t) return;
      setStuck(
        t.getBoundingClientRect().top - s.getBoundingClientRect().bottom > 1,
      );
    };
    const schedule = () => {
      if (frame === 0) frame = window.requestAnimationFrame(update);
    };
    schedule();
    window.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    return () => {
      if (frame !== 0) window.cancelAnimationFrame(frame);
      window.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
    };
  }, [sticky]);

  return { ref, sentinelRef, sticky, stuck: sticky && stuck };
}
