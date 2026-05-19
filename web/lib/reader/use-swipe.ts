import { useGesture } from "@use-gesture/react";
import type { RefObject } from "react";
import type { Direction, ViewMode } from "@/lib/reader/detect";

const SWIPE_THRESHOLD_PX = 30;

/**
 * Horizontal swipe-to-flip on the reader pane. Webtoon mode opts out
 * (vertical scroll is the native interaction). Pinch-zoomed viewports
 * also opt out so single-finger panning doesn't accidentally turn the
 * page out from under the reader.
 *
 * `enabled` is the caller's gate — `false` while the user is mid-
 * highlight or has a pending marker editor open, since
 * `@use-gesture/react` attaches native pointer listeners BEFORE
 * React's synthetic handlers on the SVG overlay, so a horizontal drag
 * in highlight mode was previously being interpreted as a page-flip.
 * Switching off the gesture entirely is cleaner than racing
 * `stopPropagation` on the native handlers.
 */
export function useReaderSwipe(opts: {
  target: RefObject<HTMLDivElement | null>;
  enabled: boolean;
  viewMode: ViewMode;
  direction: Direction;
  onNext: () => void;
  onPrev: () => void;
}): void {
  const { target, enabled, viewMode, direction, onNext, onPrev } = opts;
  useGesture(
    {
      onDragEnd: ({ movement: [mx], cancel }) => {
        if (viewMode === "webtoon") {
          cancel();
          return;
        }
        if (typeof window !== "undefined") {
          const scale = window.visualViewport?.scale ?? 1;
          if (scale > 1.05) return;
        }
        if (Math.abs(mx) < SWIPE_THRESHOLD_PX) return;
        // Swipe-right (positive mx) → previous page in LTR, next in RTL.
        const swipeIsForward = direction === "rtl" ? mx > 0 : mx < 0;
        if (swipeIsForward) onNext();
        else onPrev();
      },
    },
    {
      target,
      drag: {
        axis: "x",
        filterTaps: true,
        threshold: 10,
        enabled,
      },
      eventOptions: { passive: false },
    },
  );
}
