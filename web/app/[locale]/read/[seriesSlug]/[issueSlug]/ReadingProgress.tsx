import { readingPercent } from "@/lib/reader/fullscreen";
import type { Direction } from "@/lib/reader/detect";
import { useReaderStore } from "@/lib/reader/store";

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
 *
 * Mirrored in RTL (audit UX-5): a right-to-left read advances from the
 * right edge, matching the page-turn direction and the page strip.
 * Direction defaults to the reader store's so the chrome needn't thread
 * it through; pass it explicitly to override.
 */
export function ReadingProgress({
  current,
  total,
  direction,
}: {
  current: number;
  total: number;
  direction?: Direction;
}) {
  const storeDirection = useReaderStore((s) => s.direction);
  const rtl = (direction ?? storeDirection) === "rtl";
  const pct = readingPercent(current, total);
  return (
    <div
      role="progressbar"
      aria-label="Reading progress"
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
      data-direction={rtl ? "rtl" : "ltr"}
      className="pointer-events-none absolute inset-x-0 bottom-0 h-0.5 bg-neutral-800/40"
    >
      <span
        aria-hidden="true"
        className={`bg-accent block h-full transition-[width] duration-300 ease-out motion-reduce:transition-none ${
          rtl ? "ml-auto origin-right" : "origin-left"
        }`}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}
