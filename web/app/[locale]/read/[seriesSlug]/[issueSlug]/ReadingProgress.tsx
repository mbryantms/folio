"use client";

import { useReaderStore } from "@/lib/reader/store";
import { readingPercent } from "@/lib/reader/fullscreen";

/**
 * Thin reading-progress bar pinned to the top edge of the viewport. Visible
 * only while the chrome is shown — when chrome auto-hides, the bar fades
 * out with it so the user is left with just the comic. Width transitions
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
  const chromeVisible = useReaderStore((s) => s.chromeVisible);
  const pct = readingPercent(current, total);
  return (
    <div
      role="progressbar"
      aria-label="Reading progress"
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-hidden={chromeVisible ? undefined : true}
      data-state={chromeVisible ? "open" : "closed"}
      className="pointer-events-none fixed inset-x-0 top-0 z-40 h-0.5 bg-neutral-800/40 transition-opacity duration-200 ease-out data-[state=closed]:opacity-0 data-[state=open]:opacity-100 motion-reduce:transition-none"
    >
      <span
        aria-hidden="true"
        className="bg-accent block h-full origin-left transition-[width] duration-300 ease-out motion-reduce:transition-none"
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}
