"use client";

import { useLayoutEffect, useRef } from "react";

import { useReducedMotion } from "@/lib/visual-effects";

/** Last width drawn per key during this session (client navigations keep
 *  module state; a full reload starts fresh). */
const lastShown = new Map<string, number>();

/**
 * Inner fill of a card's progress bar (folio-flair-1.0 "Animated progress
 * on return", standard). When a card re-mounts with a different percent
 * than it last showed (you read a few pages and came back), the bar grows
 * from the old value to the new one instead of jumping. First sight and
 * reduced motion draw the final width straight away.
 */
export function ProgressFill({
  id,
  percent,
  className = "bg-primary h-full",
}: {
  /** Stable per-issue key, e.g. the issue id. */
  id: string;
  percent: number;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();
  const prev = lastShown.get(id);

  useLayoutEffect(() => {
    const el = ref.current;
    lastShown.set(id, percent);
    if (!el || reduced || prev === undefined || prev === percent) return;
    el.style.transition = "none";
    el.style.width = `${prev}%`;
    // Force the start width to apply before animating to the target.
    void el.offsetWidth;
    el.style.transition = "width 700ms cubic-bezier(0.2, 0.8, 0.2, 1) 150ms";
    el.style.width = `${percent}%`;
  }, [id, percent, prev, reduced]);

  return (
    <div
      ref={ref}
      className={`${className} transition-[width]`}
      style={{ width: `${percent}%` }}
    />
  );
}
