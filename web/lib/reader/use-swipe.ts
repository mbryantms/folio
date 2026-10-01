import { useGesture } from "@use-gesture/react";
import { useEffect, useMemo, useRef, type RefObject } from "react";
import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import type { Direction, ViewMode } from "@/lib/reader/detect";

const SWIPE_THRESHOLD_PX = 30;
/** Native pinch-zoom past this scale means a one-finger drag pans the
 *  zoomed viewport and must not turn the page. */
const PINCH_ZOOM_SCALE_MIN = 1.05;

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
 * Opt-in attribute for the reader's page surface (the tap-zone grid and
 * the webtoon tap layer) where the iOS standalone edge-back guard below
 * applies. Opt-in rather than "everything under the container" so a
 * touch on real chrome controls parked at the left edge (the exit
 * button) keeps its click.
 */
export const EDGE_GUARD_ATTR = "data-edge-guard";

/** Width (px) of the left-edge strip WebKit claims for its
 *  swipe-back-to-previous-page gesture. */
export const EDGE_BACK_INSET_PX = 24;

/** Max travel / duration for a touch in the edge strip to still count as
 *  a tap (the guard's `preventDefault` suppresses the synthesized click,
 *  so the guard re-dispatches the tap itself). */
const EDGE_TAP_MAX_TRAVEL_PX = 10;
const EDGE_TAP_MAX_MS = 500;

/** True when a touch starting at `clientX` falls in the left-edge strip
 *  iOS reserves for edge-swipe-back. */
export function inEdgeBackInset(
  clientX: number,
  inset: number = EDGE_BACK_INSET_PX,
): boolean {
  return clientX >= 0 && clientX < inset;
}

/** Whether a touch that started in the edge strip ended as a tap. */
export function isEdgeTap(
  start: { x: number; y: number; t: number },
  end: { x: number; y: number; t: number },
): boolean {
  return (
    end.t - start.t <= EDGE_TAP_MAX_MS &&
    Math.hypot(end.x - start.x, end.y - start.y) <= EDGE_TAP_MAX_TRAVEL_PX
  );
}

/**
 * Installed iOS / iPadOS web app (Home Screen). `navigator.standalone`
 * only exists on Apple WebKit, so this is false everywhere else —
 * including standalone Android / desktop PWAs, which have no edge-back
 * gesture to fight.
 */
export function isIosStandalone(): boolean {
  if (typeof window === "undefined") return false;
  const nav = window.navigator as Navigator & { standalone?: boolean };
  return nav.standalone === true;
}

/**
 * Maps a completed horizontal drag to a page-turn. Shared by the
 * primary gesture binding and the pointer-stream fallback below so
 * both agree on threshold and reading-direction mapping.
 * Swipe-right (positive mx) → previous page in LTR, next in RTL.
 */
export function swipeAction(
  mx: number,
  direction: Direction,
): "next" | "prev" | null {
  if (Math.abs(mx) < SWIPE_THRESHOLD_PX) return null;
  const swipeIsForward = direction === "rtl" ? mx > 0 : mx < 0;
  return swipeIsForward ? "next" : "prev";
}

/**
 * True when the user is genuinely pinch-zoomed (native browser zoom),
 * in which case a one-finger drag pans the zoomed viewport and a
 * swipe must not turn the page out from under the reader.
 *
 * `visualViewport.scale` alone is not trustworthy for this: iOS
 * standalone PWAs are known to resume from the background with stale
 * visualViewport state that only a force-quit clears (same bug family
 * as the stuck-viewport-height keyboard bug), and a stale scale > 1
 * here silently ate every swipe — the drag ran to completion and the
 * lift declined to act, while tap-to-turn kept working. So corroborate
 * the scale with the layout-vs-visual width ratio: when actually
 * pinch-zoomed the visual viewport is narrower than the layout
 * viewport by exactly the scale factor; a scale reading the widths
 * don't back is stale and is ignored.
 */
export function isPinchZoomed(
  scale: number,
  visualWidth: number,
  layoutWidth: number,
): boolean {
  if (scale <= PINCH_ZOOM_SCALE_MIN) return false;
  // No usable widths to corroborate with — trust the scale reading.
  if (visualWidth <= 0 || layoutWidth <= 0) return true;
  return layoutWidth / visualWidth > PINCH_ZOOM_SCALE_MIN;
}

function pinchZoomedNow(): boolean {
  if (typeof window === "undefined") return false;
  const vv = window.visualViewport;
  if (!vv) return false;
  return isPinchZoomed(
    vv.scale,
    vv.width,
    document.documentElement.clientWidth,
  );
}

type ReaderGestureOpts = {
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
};

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
export function useReaderGestures(opts: ReaderGestureOpts): void {
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
        if (pinchZoomedNow()) return;
        const action = swipeAction(mx, direction);
        if (action === "next") onNext();
        else if (action === "prev") onPrev();
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

  // Kept fresh so the fallback listeners below (bound once) read the
  // current gate state at event time instead of a stale closure.
  const optsRef = useRef(opts);
  useEffect(() => {
    optsRef.current = opts;
  });

  // Resume-wedge fallback — the sequel to the coarse-pointer touch
  // binding above. The standalone-PWA WebKit defect family starves one
  // input stream after a lifecycle event: an in-app navigation can kill
  // the captured *pointer* stream (which the touch binding dodges), and
  // a background/reopen cycle can come back with the *touch* stream
  // starved instead — touchmove stops arriving while WebKit still
  // synthesizes clicks, so tap-to-turn keeps working and swipe goes
  // silently dead until force-quit.
  //
  // This fallback watches the pointer stream in parallel and completes
  // a page-turn ONLY for a drag during which not a single touchmove was
  // delivered — i.e. exactly when the touch binding cannot have seen
  // the swipe. On a healthy device every past-threshold finger drag
  // produces touchmoves, so the fallback never acts and can't
  // double-turn. It deliberately handles page-turn only (not pan): it
  // has no move stream to track a pan with, and TapZones still covers
  // navigation while zoomed.
  //
  // No listener here uses pointer capture, and the up/cancel listeners
  // ride on `window` — staying clear of the captured-stream machinery
  // the original wedge starves. This is not a second drag *claim* (the
  // audit C4 rule): it never preventDefaults, never cancels, and stands
  // down whenever the primary binding could have run.
  useEffect(() => {
    // Fine-pointer devices already ride the pointer path in the primary
    // binding; the fallback only pairs with the touch binding.
    if (!touchEvents) return;
    const el = target.current;
    if (!el) return;
    let gesture: {
      id: number;
      x: number;
      y: number;
      sawTouchMove: boolean;
      ignored: boolean;
    } | null = null;
    const onTouchMove = () => {
      if (gesture) gesture.sawTouchMove = true;
    };
    const onDown = (e: PointerEvent) => {
      if (e.pointerType !== "touch") return;
      if (gesture !== null) {
        // Second concurrent finger — a pinch, not a swipe. Abort.
        gesture = null;
        return;
      }
      gesture = {
        id: e.pointerId,
        x: e.clientX,
        y: e.clientY,
        sawTouchMove: false,
        ignored: startedInIgnoredChrome(e),
      };
    };
    const onCancel = () => {
      gesture = null;
    };
    const onUp = (e: PointerEvent) => {
      const g = gesture;
      gesture = null;
      if (!g || e.pointerId !== g.id) return;
      // Touch stream is alive → the primary binding owns this drag.
      if (g.sawTouchMove || g.ignored) return;
      const { enabled, viewMode, direction, panActive, onNext, onPrev } =
        optsRef.current;
      if (!enabled || panActive || viewMode === "webtoon") return;
      if (pinchZoomedNow()) return;
      const dx = e.clientX - g.x;
      const dy = e.clientY - g.y;
      // Vertical intent belongs to native scroll.
      if (Math.abs(dx) <= Math.abs(dy)) return;
      const action = swipeAction(dx, direction);
      if (action === "next") onNext();
      else if (action === "prev") onPrev();
    };
    el.addEventListener("pointerdown", onDown, { passive: true });
    window.addEventListener("touchmove", onTouchMove, {
      passive: true,
      capture: true,
    });
    window.addEventListener("pointerup", onUp, {
      passive: true,
      capture: true,
    });
    window.addEventListener("pointercancel", onCancel, {
      passive: true,
      capture: true,
    });
    return () => {
      el.removeEventListener("pointerdown", onDown);
      window.removeEventListener("touchmove", onTouchMove, { capture: true });
      window.removeEventListener("pointerup", onUp, { capture: true });
      window.removeEventListener("pointercancel", onCancel, {
        capture: true,
      });
    };
  }, [touchEvents, target]);

  // iOS standalone edge-back guard (audit UX-10). In a Home-Screen web
  // app WebKit turns a rightward swipe that starts at the left screen
  // edge into history-back — which exits the reader mid-issue instead of
  // turning to the previous page. `preventDefault` on a `touchstart` in
  // that strip is the one lever WebKit honours to suppress it; the swipe
  // itself still reaches the drag binding above (touchmove is not
  // cancelled), so an edge swipe now turns the page like any other.
  // Cancelling touchstart also swallows the synthesized click, so a
  // plain tap in the strip is re-dispatched here as the left tap zone.
  // Scoped to the opted-in page surface (EDGE_GUARD_ATTR) so chrome
  // buttons at the edge keep working, and to iOS standalone only —
  // Safari-in-browser owns its own back gesture and nothing else has one.
  useEffect(() => {
    if (!isIosStandalone()) return;
    const el = target.current;
    if (!el) return;
    let start: { id: number; x: number; y: number; t: number } | null = null;
    const onTouchStart = (e: TouchEvent) => {
      start = null;
      if (e.touches.length !== 1) return;
      const touch = e.touches[0]!;
      if (!inEdgeBackInset(touch.clientX)) return;
      if (
        !(e.target instanceof Element) ||
        e.target.closest(`[${EDGE_GUARD_ATTR}]`) === null
      ) {
        return;
      }
      e.preventDefault();
      start = {
        id: touch.identifier,
        x: touch.clientX,
        y: touch.clientY,
        t: e.timeStamp,
      };
    };
    const onTouchEnd = (e: TouchEvent) => {
      const s = start;
      start = null;
      if (!s) return;
      const touch = Array.from(e.changedTouches).find(
        (t) => t.identifier === s.id,
      );
      if (!touch) return;
      const end = { x: touch.clientX, y: touch.clientY, t: e.timeStamp };
      if (!isEdgeTap(s, end)) return;
      const { enabled, viewMode, direction, onNext, onPrev } = optsRef.current;
      if (!enabled || viewMode === "webtoon") return;
      // Left tap zone: previous page in LTR, next in RTL.
      if (direction === "rtl") onNext();
      else onPrev();
    };
    const onTouchCancel = () => {
      start = null;
    };
    el.addEventListener("touchstart", onTouchStart, { passive: false });
    el.addEventListener("touchend", onTouchEnd, { passive: true });
    el.addEventListener("touchcancel", onTouchCancel, { passive: true });
    return () => {
      el.removeEventListener("touchstart", onTouchStart);
      el.removeEventListener("touchend", onTouchEnd);
      el.removeEventListener("touchcancel", onTouchCancel);
    };
  }, [target]);
}
