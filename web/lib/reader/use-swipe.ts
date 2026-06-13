import { useGesture } from "@use-gesture/react";
import type { RefObject } from "react";
import type { Direction, ViewMode } from "@/lib/reader/detect";

const SWIPE_THRESHOLD_PX = 30;

/**
 * The reader's single drag-gesture claim layer (audit C4 + C9). One
 * `useGesture` on the reader pane — two instances would race each other's
 * native listeners, the exact ordering bug the `enabled` gate guards.
 *
 * The gesture is attached to the outer reader container, so drags bubble
 * up to it even though the `TapZones` overlay (`z-10`) is the pointer
 * target — which is why the pan must live HERE and not on a wrapper
 * beneath the zones (a wrapper never receives the drag).
 *
 * Two modes, chosen by `panActive`:
 *  - **pan** (zoomed, or a fit=height/original page overflowing the
 *    viewport): the drag pans the page via `onPan` (both axes); no
 *    page-turn. Page-turn is still reachable by tapping the `TapZones`.
 *  - **page-turn** (default): a horizontal drag past the threshold flips
 *    the page. Webtoon opts out (vertical scroll is native); a
 *    pinch-zoomed visual viewport opts out so a single-finger pan
 *    doesn't turn the page out from under the reader.
 *
 * `enabled` is the caller's gate — `false` while mid-highlight / pending
 * marker editor (the SVG overlay's native listeners would otherwise read
 * a horizontal drag as a page-flip).
 */
export function useReaderGestures(opts: {
  target: RefObject<HTMLDivElement | null>;
  enabled: boolean;
  viewMode: ViewMode;
  direction: Direction;
  onNext: () => void;
  onPrev: () => void;
  /** When true the drag pans the page instead of turning it. */
  panActive: boolean;
  /** Drag start while panning — caller snapshots the current offset. */
  onPanStart: () => void;
  /** Movement (px, from drag start) while panning. */
  onPan: (dx: number, dy: number) => void;
}): void {
  const {
    target,
    enabled,
    viewMode,
    direction,
    onNext,
    onPrev,
    panActive,
    onPanStart,
    onPan,
  } = opts;
  useGesture(
    {
      onDragStart: () => {
        if (panActive) onPanStart();
      },
      onDrag: ({ movement: [mx, my] }) => {
        if (panActive) onPan(mx, my);
      },
      onDragEnd: ({ movement: [mx], cancel }) => {
        // Panning consumed the drag; the offset is already applied.
        if (panActive) return;
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
        // Lock to the horizontal axis for page-turn (so native vertical
        // scroll is preserved); free both axes while panning.
        axis: panActive ? undefined : "x",
        filterTaps: true,
        threshold: 10,
        enabled,
      },
      eventOptions: { passive: false },
    },
  );
}
