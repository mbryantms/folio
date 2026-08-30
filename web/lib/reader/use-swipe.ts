import { useGesture } from "@use-gesture/react";
import { useMemo, type RefObject } from "react";
import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import type { Direction, ViewMode } from "@/lib/reader/detect";

const SWIPE_THRESHOLD_PX = 30;

/**
 * Opt-out attribute for chrome surfaces nested inside the reader's
 * gesture container that own their own horizontal touch interaction
 * (today: the PageStrip's scrollable thumbnail rail). A drag that
 * starts inside a subtree carrying this attribute is ignored by every
 * drag handler below — no page-turn, no pan.
 *
 * Why an explicit opt-out: the touch-event binding (coarse-pointer
 * devices, see coarse-pointer.ts) changed who wins when a drag starts
 * on a scrollable chrome child. With pointer events, the browser
 * fired `pointercancel` the moment native scroll claimed the touch,
 * killing the drag before it could act. Touch events have no such
 * courtesy — WebKit keeps delivering `touchmove` to the start target
 * throughout a native scroll — so a strip scroll also ran the
 * container's drag to completion: the lift turned the page, and the
 * page-change recenter effect snapped the strip right back, reading
 * as "the strip won't scroll".
 */
export const SWIPE_IGNORE_ATTR = "data-swipe-ignore";

/** True when the drag's originating element sits inside an opted-out
 *  chrome subtree. `event.target` is stable across the whole drag in
 *  both bindings: touch events keep the touch-start target for every
 *  `touchmove`/`touchend`, and the pointer path captures the pointer
 *  to it. */
function startedInIgnoredChrome(event: { target: EventTarget | null }) {
  return (
    event.target instanceof Element &&
    event.target.closest(`[${SWIPE_IGNORE_ATTR}]`) !== null
  );
}

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
  // Standalone-PWA WebKit wedge: bind through touch events on
  // coarse-pointer devices (see lib/reader/coarse-pointer.ts). With
  // `drag.pointer.touch` set, behavior is otherwise identical —
  // `touch-action: pan-y pinch-zoom` on the container still lets the
  // browser claim vertical scroll + pinch natively (the drag gets a
  // `touchcancel` exactly like it got a `pointercancel`). Input class
  // can't change mid-session; resolve once per mount.
  const touchEvents = useMemo(() => primaryPointerIsCoarse(), []);
  useGesture(
    {
      onDragStart: ({ event }) => {
        if (startedInIgnoredChrome(event)) return;
        if (panActive) onPanStart();
      },
      onDrag: ({ event, movement: [mx, my] }) => {
        if (panActive && !startedInIgnoredChrome(event)) onPan(mx, my);
      },
      onDragEnd: ({ event, movement: [mx], cancel }) => {
        if (startedInIgnoredChrome(event)) return;
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
        // Touch-event binding on coarse-pointer devices — dodges the
        // standalone-PWA pointer-capture wedge (coarse-pointer.ts).
        pointer: { touch: touchEvents },
      },
      eventOptions: { passive: false },
    },
  );
}
