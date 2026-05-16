"use client";

import { readingPercent } from "@/lib/reader/fullscreen";

/**
 * Thin reading-progress bar sitting along the bottom edge of the top
 * chrome bar. Rendered as an absolute child of `<ReaderChrome>`'s
 * `<header>`, so it inherits the chrome's slide-up transition when
 * auto-hide kicks in — no separate fade needed here. Width transitions
 * smoothly on page change so flipping pages reads as a small slide.
 *
 * Caller decides what `current` / `total` mean: page index in single/
 * webtoon mode, group index in double-page mode (so a spread doesn't
 * under-count visual progress).
 */
export function ReadingProgress({
  current,
  total,
}: {
  current: number;
  total: number;
}) {
  const pct = readingPercent(current, total);
  return (
    <div
      role="progressbar"
      aria-label="Reading progress"
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
      className="pointer-events-none absolute inset-x-0 bottom-0 h-0.5 bg-neutral-800/40"
    >
      <span
        aria-hidden="true"
        className="bg-accent block h-full origin-left transition-[width] duration-300 ease-out motion-reduce:transition-none"
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}
