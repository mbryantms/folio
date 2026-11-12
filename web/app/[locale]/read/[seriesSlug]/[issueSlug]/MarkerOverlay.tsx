"use client";

import * as React from "react";
import { isCancelledError, useQueryClient } from "@tanstack/react-query";
import {
  BookmarkCheck,
  Loader2,
  MessageSquareText,
  RotateCcw,
  Star,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/folio/tooltip";
import { useIssueMarkers, useIssuePageTextRegions } from "@/lib/api/queries";
import {
  useDeleteMarker,
  useRestoreMarkers,
} from "@/lib/api/mutations/markers";
import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import { showUndoToast } from "@/lib/ui/undo-toast";
import {
  useReaderStore,
  type MarkerMode,
  type PendingMarker,
} from "@/lib/reader/store";
import { toOcrRequestError } from "@/lib/reader/ocr-errors";
import {
  measuredOriginalSize,
  pagePixelsToRegion,
  resolvePageSize,
  type PageSize,
} from "@/lib/reader/page-size";
import {
  PAGE_TEXT_GC_MS,
  WEBTOON_DETECT_DELAY_MS,
  hitTestBox,
  readingOrderRegions,
} from "@/lib/reader/text-regions";
import { useSettledFlag } from "@/lib/reader/use-settled-flag";
import { cn } from "@/lib/utils";
import type {
  MarkerKind,
  MarkerRegion,
  MarkerView,
  TextRegionView,
} from "@/lib/api/types";
import type { OcrResult } from "./marker-selection";

/** Pointer travel (percent of image size) below which a release
 *  counts as a tap rather than a drag. ~0.8% of a 1000px-wide page
 *  is 8px — forgiving enough for touch, far below any real drag. */
const TAP_TRAVEL_MAX = 0.8;

/** Webtoon + touch: how long a finger must rest before a drag starts.
 *  Until then the page scrolls natively (the overlay is `pan-y` there);
 *  a quick tap still taps a box. */
const TOUCH_HOLD_MS = 350;
/** Finger travel (CSS px) that turns a pending press into a scroll. */
const TOUCH_SCROLL_SLOP_PX = 10;

/** What a capture says when the server read the area and found nothing. */
const NO_TEXT_MESSAGE =
  "No text found in that area. The highlight editor opens without captured text; you can type it in.";
/** How long the detection status stays empty after it mounts, so the
 *  screen reader has registered the live region before it is filled. */
const LIVE_REGION_FILL_DELAY_MS = 150;
/** Longest accessible name a saved region's proxy carries. */
const REGION_LABEL_MAX = 240;

/** Per-kind colors for the SVG rect markers. Translucent fill + solid
 *  stroke so highlights remain readable over the page content
 *  underneath. */
const KIND_FILL: Record<MarkerKind, string> = {
  bookmark: "rgba(245, 158, 11, 0.18)",
  note: "rgba(59, 130, 246, 0.18)",
  // Favorites are always page-level (no region), so these only apply
  // if a future surface introduces region favorites. Reuse the star
  // amber palette to stay consistent with the chrome.
  favorite: "rgba(245, 158, 11, 0.18)",
  highlight: "rgba(234, 179, 8, 0.22)",
};
const KIND_STROKE: Record<MarkerKind, string> = {
  bookmark: "rgb(245, 158, 11)",
  note: "rgb(59, 130, 246)",
  favorite: "rgb(245, 158, 11)",
  highlight: "rgb(234, 179, 8)",
};

const KIND_PIN_BG: Record<MarkerKind, string> = {
  bookmark: "bg-amber-500/90 text-white",
  note: "bg-blue-500/90 text-white",
  favorite: "bg-amber-500/90 text-white",
  highlight: "bg-yellow-500/90 text-black",
};

/** Marker overlay anchored on a single PageImage wrapper.
 *
 *  - Region markers render as percent-positioned SVG rects (the SVG
 *    has `viewBox="0 0 100 100"` + `preserveAspectRatio="none"` so the
 *    coords are CSS percent and the layout engine handles
 *    resize/zoom/fit-mode without us recomputing anything).
 *  - Page-level markers (region NULL — bookmarks, notes on the whole
 *    page) render as **HTML siblings** of the SVG, stacked in the
 *    top-right corner. Rendering pins inside the SVG used to blow them
 *    up dramatically because the viewBox's non-uniform stretch
 *    scales any HTML inside `<foreignObject>` by the same factor;
 *    keeping them outside fixes that.
 *  - When the reader is in a `select-*` mode the overlay turns into a
 *    drag-capture surface that pushes a `PendingMarker` onto the store
 *    on release. */
export function MarkerOverlay({
  issueId,
  pageIndex,
  imgRef,
  serverPageSize,
}: {
  issueId: string;
  pageIndex: number;
  /** The actually-rendered `<img>` element. Used to compute the SVG's
   *  position + dimensions so it overlays the image exactly. Pointer
   *  coordinates are taken relative to this image's bounds.
   *
   *  The SVG's positioning context (its CSS containing block) is
   *  resolved at runtime by walking up from the SVG's own DOM
   *  parent until we find an element with `position != static`. This
   *  avoids a React 19 commit-phase race where a parent's
   *  `ref={...}` attribute is still null at the time a descendant's
   *  layout effect fires — the bug that left webtoon-mode overlays
   *  stuck at `inset:0`. */
  imgRef: React.RefObject<HTMLImageElement | null>;
  /** The page's pixel size from the server's page list
   *  (`image_width` / `image_height`), or `null` when the scanner did not
   *  record it. One of the sources of `resolvePageSize`; the `<img>`'s own
   *  natural size is NOT passed in, because with a `srcSet` it follows the
   *  loaded variant and the layout rather than the original page. */
  serverPageSize: PageSize | null;
}) {
  const markerMode = useReaderStore((s) => s.markerMode);
  const setMarkerMode = useReaderStore((s) => s.setMarkerMode);
  const beginMarkerEdit = useReaderStore((s) => s.beginMarkerEdit);
  // User-toggled "read without distractions" — hides saved rects + pins
  // but leaves the drag-capture surface live when the user explicitly
  // enters a select-* mode (clicking the chrome's marker menu also
  // un-hides; see the settings popover wiring).
  const markersHidden = useReaderStore((s) => s.markersHidden);
  // Page pins live in the top-right of the page wrapper. When the
  // reader chrome (top bar) is visible — which often happens at
  // viewport-y=0 — the chrome sits over the pin column. Shift the
  // column down by the chrome's height (~56px) so it never gets
  // covered. Use the same store flag the chrome itself reads so the
  // animation reverses cleanly when the chrome auto-hides.
  const chromeVisible = useReaderStore((s) => s.chromeVisible);
  const chromePinned = useReaderStore((s) => s.chromePinned);
  const chromeShowing = chromeVisible || chromePinned;
  const issueQuery = useIssueMarkers(issueId);

  // Bubble outlines (OCR rework 1.0): when the reader enters
  // text-capture mode, fetch the page's detected text regions and
  // render them as tappable outlines. Progressive enhancement — the
  // drag surface is live immediately; outlines appear when the
  // detector responds (instant on a warm cache, seconds cold).
  //
  // The visibility gate matters: webtoon mode mounts an overlay per
  // page, so without it entering text mode would fan out a detector
  // run for every page in the issue. `currentPage` covers single +
  // webtoon; double-page panes are `currentPage`'s group, which is
  // always within ±1.
  const currentPage = useReaderStore((s) => s.currentPage);
  const viewMode = useReaderStore((s) => s.viewMode);
  const pageVisible =
    viewMode === "double"
      ? Math.abs(pageIndex - currentPage) <= 1
      : pageIndex === currentPage;
  // Webtoon: only a page that stays current for a moment is detected, so
  // scrolling through the issue in capture mode does not start a
  // detection for every page passed (the bucket allows 15 a minute).
  const detectEnabled = useSettledFlag(
    markerMode === "select-text" && pageVisible,
    viewMode === "webtoon" ? WEBTOON_DETECT_DELAY_MS : 0,
  );
  const regionsQuery = useIssuePageTextRegions(
    issueId,
    pageIndex,
    detectEnabled,
    {
      gcTime: PAGE_TEXT_GC_MS,
    },
  );
  // THE text boxes of this page (`text-regions.ts`): nested line boxes
  // collapsed into their block, in reading order. The outlines drawn, the
  // pointer hit-test and the keyboard proxies all use this one set, and
  // the page-text panel reads the same set, so a tap, a key press and the
  // panel read the same box.
  const direction = useReaderStore((s) => s.direction);
  const textBoxes = React.useMemo(
    () =>
      markerMode === "select-text"
        ? readingOrderRegions(regionsQuery.data?.regions ?? [], direction)
        : [],
    [markerMode, regionsQuery.data, direction],
  );
  // Bubble the user is hovering (pointer) and the one whose OCR is
  // in flight after a tap. Outline rects are `pointer-events: none`
  // — hit-testing happens in the SVG's own pointer handlers so the
  // drag-capture path keeps exclusive ownership of pointer capture.
  const [hoverRegion, setHoverRegion] = React.useState<TextRegionView | null>(
    null,
  );
  const [busyRegion, setBusyRegion] = React.useState<TextRegionView | null>(
    null,
  );
  React.useEffect(() => {
    if (markerMode !== "select-text") {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setHoverRegion(null);
    }
  }, [markerMode]);

  // One capture at a time (D4). The token is a ref, not state, so two
  // taps inside one render frame still see each other. Leaving the mode
  // (Esc, the pill's Cancel) drops the token: the in-flight request still
  // finishes, but its answer is ignored and no editor opens.
  const queryClient = useQueryClient();
  const captureRef = React.useRef<symbol | null>(null);
  const captureToastRef = React.useRef<string | number | null>(null);
  // The last failure toast (its Try again re-runs a capture), dismissed
  // when the mode ends so a stale Try again can't open an editor later.
  const failureToastRef = React.useRef<string | number | null>(null);
  const cancelCapture = React.useCallback(() => {
    captureRef.current = null;
    for (const ref of [captureToastRef, failureToastRef]) {
      if (ref.current != null) {
        toast.dismiss(ref.current);
        ref.current = null;
      }
    }
    setBusyRegion(null);
  }, []);
  React.useEffect(() => {
    if (markerMode === "select-text") return;
    // Leaving the mode drops the in-flight capture (clears its busy
    // outline); bounded by the mode check above.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    cancelCapture();
  }, [markerMode, cancelCapture]);
  React.useEffect(() => () => cancelCapture(), [cancelCapture]);

  /** The page's real pixel size, from the one resolver (`page-size.ts`):
   *  the detection's decode size, else the server's page list, else the
   *  `<img>` only when it loaded the original bytes. */
  function pageSizeNow(): PageSize | null {
    const d = regionsQuery.data;
    return resolvePageSize({
      detected: d ? { width: d.page_w, height: d.page_h } : null,
      server: serverPageSize,
      measured: measuredOriginalSize(imgRef.current),
    });
  }

  const pageMarkers = React.useMemo(
    () =>
      (issueQuery.data?.items ?? []).filter((m) => m.page_index === pageIndex),
    [issueQuery.data, pageIndex],
  );
  // Split: region rects belong in the SVG; page-level pins render as
  // HTML so the SVG viewBox can't blow them up. When `markersHidden`
  // is true, hide the saved markers entirely — but only when the user
  // isn't actively dragging a new one (the live drag preview is always
  // visible since it's the user's current intent).
  const regionMarkers = markersHidden
    ? []
    : pageMarkers.filter((m) => m.region);
  const pagePins = markersHidden ? [] : pageMarkers.filter((m) => !m.region);

  // Open the editor on an existing marker, preserving its region (so the
  // SVG rect and the keyboard-focusable proxy below share one path).
  const editExistingMarker = (marker: MarkerView) =>
    beginMarkerEdit(
      {
        kind: marker.kind,
        page_index: marker.page_index,
        region: marker.region ?? null,
        selection: marker.selection ?? null,
        body: marker.body ?? "",
        is_favorite: marker.is_favorite,
        tags: marker.tags,
      },
      marker.id,
    );

  const [drag, setDrag] = React.useState<DragState | null>(null);
  // Standalone-PWA WebKit wedge (see lib/reader/coarse-pointer.ts):
  // on coarse-pointer devices the select drag binds through touch
  // events instead of pointer events. Exclusive binding — attaching
  // both sets would double-run the drag (and double-open the editor)
  // on devices where the pointer stream still works. Input class
  // can't change mid-session; resolve once per mount.
  const touchInput = React.useMemo(() => primaryPointerIsCoarse(), []);
  // Track the image's position + size within the wrapper so the SVG
  // overlay aligns with the rendered image (not the wider flex
  // container that holds it). Recomputed whenever the wrapper or
  // image element resizes — covers fit-mode flips, viewport resizes,
  // and async-loaded images that change dimensions mid-render.
  //
  // CRITICAL: `pageIndex` + `issueId` are in the dep list so the
  // observer rewires whenever the parent swaps the `<img>` element
  // (PageImage is keyed on those). Without that, the ResizeObserver
  // stays attached to the previous issue's now-unmounted img, the
  // stored `imgRect` keeps stale measurements, and a saved highlight
  // in a different issue renders far from its real position.
  const [imgRect, setImgRect] = React.useState<{
    top: number;
    left: number;
    width: number;
    height: number;
  } | null>(null);
  // useLayoutEffect (not useEffect) so the initial measurement lands
  // before the browser paints — avoids a frame where the SVG is at
  // the wrong place because `imgRect` hasn't updated yet from the
  // previous issue's bounds.
  //
  // Positioning approach: read `img.offsetTop/Left/Width/Height`,
  // not `getBoundingClientRect`. The two siblings (img + svg) share
  // the same `offsetParent`, so the offsets align them in the same
  // coordinate space — including during the reader's slide
  // page-turn, which puts a `transform` on a static ancestor and
  // would otherwise make `getBoundingClientRect` (viewport-relative)
  // disagree with the SVG's actual containing block (the transformed
  // ancestor, per CSS containing-block rules). Using `offsetLeft`
  // sidesteps that entirely: it never includes ancestor transforms.
  React.useLayoutEffect(() => {
    const img = imgRef.current;
    if (!img) {
      setImgRect(null);
      return;
    }
    // Clear immediately on rebind so a stale rect from the previous
    // issue can't flash through during the (sub-frame) window before
    // `recompute()` writes the new value.
    setImgRect(null);
    const recompute = () => {
      // Skip while the img hasn't laid out yet — emitting zeros
      // would render the overlay as a 0×0 box and the first real
      // measurement on the next observer tick would create a
      // visible jump.
      if (img.offsetWidth <= 0 || img.offsetHeight <= 0) return;
      setImgRect({
        top: img.offsetTop,
        left: img.offsetLeft,
        width: img.offsetWidth,
        height: img.offsetHeight,
      });
    };
    recompute();
    const ro = new ResizeObserver(() => recompute());
    ro.observe(img);
    // Browsers don't fire ResizeObserver for late-arriving image
    // bytes that change the `<img>` element's intrinsic size, so
    // we also hook the load event explicitly.
    img.addEventListener("load", recompute);
    // Cached-image race: when an `<img>` is already in the
    // browser's disk/memory cache, its `load` event fires BEFORE
    // this layout effect attaches the listener. Without this
    // synchronous probe the overlay's `imgRect` stays null and the
    // SVG falls back to its intrinsic-aspect fallback.
    if (img.complete && img.naturalWidth > 0) {
      recompute();
    }
    return () => {
      ro.disconnect();
      img.removeEventListener("load", recompute);
    };
    // pageIndex/issueId are the implicit identity of the rendered
    // <img> — when they change, PageImage remounts a new element and
    // we must rebind the observer to it.
  }, [imgRef, pageIndex, issueId]);

  const enabled = markerMode !== "idle";
  React.useEffect(() => {
    if (!enabled) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setDrag(null);
    }
  }, [enabled]);
  React.useEffect(() => {
    if (markerMode === "idle") return;
    const onKey = (e: KeyboardEvent) => {
      // While the marker editor (or its confirm dialog) is open, or the key
      // comes from inside any dialog (the page-text panel), Esc belongs to
      // that surface: the mode stays text capture while the editor is
      // open, and taking the key here would cost the dialog a press.
      if (
        useReaderStore.getState().pendingMarker !== null ||
        (e.target instanceof Element &&
          e.target.closest('[role="dialog"], [role="alertdialog"]'))
      ) {
        return;
      }
      if (e.key === "Escape") {
        // `stopImmediatePropagation` stops the Reader's bubble-phase
        // keybind handler from also running on this same event —
        // otherwise Escape's `quitReader` binding would route the user
        // out of the reader after we've canceled the selection. The
        // bubble-phase handler does have a `markerActive` early-return
        // guard, but it depends on React having committed the new
        // closure between the `h` and `Esc` keystrokes, which isn't
        // guaranteed under fast input. Stopping propagation here makes
        // the cancel atomic regardless of render timing.
        e.preventDefault();
        e.stopImmediatePropagation();
        setMarkerMode("idle");
        setDrag(null);
        return;
      }
      // Arrow-key nudge while the drag is in progress (mouse held
      // down). Translates the whole selection by 1% (Shift = 5%). Keeps
      // the rect clamped to [0, 100] so fine-tuning never accidentally
      // pushes it off-page.
      if (!drag) return;
      const step = e.shiftKey ? 5 : 1;
      let dx = 0;
      let dy = 0;
      switch (e.key) {
        case "ArrowLeft":
          dx = -step;
          break;
        case "ArrowRight":
          dx = step;
          break;
        case "ArrowUp":
          dy = -step;
          break;
        case "ArrowDown":
          dy = step;
          break;
        default:
          return;
      }
      e.preventDefault();
      // Same reasoning as the Escape branch above — block the bubble
      // handler so a nudge can't also flip the page on `ArrowLeft` /
      // `ArrowRight`.
      e.stopImmediatePropagation();
      setDrag((prev) => {
        if (!prev) return prev;
        const minX = Math.min(prev.startX, prev.currentX);
        const minY = Math.min(prev.startY, prev.currentY);
        const maxX = Math.max(prev.startX, prev.currentX);
        const maxY = Math.max(prev.startY, prev.currentY);
        const w = maxX - minX;
        const h = maxY - minY;
        const newMinX = clamp(minX + dx, 0, 100 - w);
        const newMinY = clamp(minY + dy, 0, 100 - h);
        return {
          startX: newMinX,
          startY: newMinY,
          currentX: newMinX + w,
          currentY: newMinY + h,
        };
      });
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () =>
      window.removeEventListener("keydown", onKey, { capture: true });
  }, [markerMode, setMarkerMode, drag]);

  // Coords are computed against the IMAGE's rect, not the wrapper's,
  // so a drag is stored in image-relative percentages regardless of
  // whether the wrapper has whitespace around the image (e.g. at
  // fit=height when the page is narrower than the viewport).
  function clientToImagePercent(clientX: number, clientY: number) {
    const rect = imgRef.current!.getBoundingClientRect();
    return {
      x: clamp(((clientX - rect.left) / rect.width) * 100, 0, 100),
      y: clamp(((clientY - rect.top) / rect.height) * 100, 0, 100),
    };
  }

  function handlePointerDown(e: React.PointerEvent<SVGSVGElement>) {
    if (!enabled || !imgRef.current) return;
    e.preventDefault();
    // A capture is reading: no second one until it lands.
    if (captureRef.current) return;
    const { x, y } = clientToImagePercent(e.clientX, e.clientY);
    setDrag({ startX: x, startY: y, currentX: x, currentY: y });
    (e.target as SVGSVGElement).setPointerCapture(e.pointerId);
  }

  function handlePointerMove(e: React.PointerEvent<SVGSVGElement>) {
    if (!imgRef.current) return;
    const { x, y } = clientToImagePercent(e.clientX, e.clientY);
    if (!drag) {
      // Idle hover in text mode: light up the box under the pointer.
      // The rects can't take CSS :hover themselves (they're
      // pointer-events: none so dragging stays unobstructed).
      if (textBoxes.length > 0) {
        setHoverRegion(hitTestBox(textBoxes, x, y));
      }
      return;
    }
    e.preventDefault();
    setDrag((prev) => (prev ? { ...prev, currentX: x, currentY: y } : prev));
  }

  function handlePointerUp(e: React.PointerEvent<SVGSVGElement>) {
    if (!drag) return;
    e.preventDefault();
    (e.target as SVGSVGElement).releasePointerCapture(e.pointerId);
    const release = drag;
    setDrag(null);
    void completeDrag(release);
  }

  // Touch-path equivalents, bound instead of the pointer handlers on
  // coarse-pointer devices (standalone-PWA WebKit wedge — see
  // lib/reader/coarse-pointer.ts). Touch events implicitly capture to
  // the element the touch started on, so there is no pointer-capture
  // dance. React's root-level touch listeners are passive, so no
  // preventDefault here. No hover path: touch has no hover, and
  // tap-to-capture hit-tests at release inside `completeDrag`.
  //
  // Paged views: the SVG carries `touch-action: none` while a select
  // mode is live, so a drag starts at once.
  // Webtoon: the SVG carries `pan-y pinch-zoom`, so the page keeps
  // scrolling under a finger. A press becomes a drag only after the
  // finger rests for TOUCH_HOLD_MS (then a native non-passive
  // `touchmove` listener cancels the scroll); a press that moves first
  // is a scroll; a quick press-and-release is a tap.
  const webtoonTouch = touchInput && viewMode === "webtoon";
  const svgRef = React.useRef<SVGSVGElement>(null);
  const pressRef = React.useRef<{
    x: number;
    y: number;
    clientX: number;
    clientY: number;
    armed: boolean;
    timer: ReturnType<typeof setTimeout> | null;
  } | null>(null);
  const clearPress = () => {
    const p = pressRef.current;
    if (p?.timer) clearTimeout(p.timer);
    pressRef.current = null;
  };
  React.useEffect(() => {
    const svg = svgRef.current;
    if (!svg || !webtoonTouch || !enabled) return;
    const onMove = (e: TouchEvent) => {
      const p = pressRef.current;
      if (!p) return;
      if (p.armed) {
        if (e.cancelable) e.preventDefault();
        return;
      }
      const t = e.touches[0];
      if (
        t &&
        Math.hypot(t.clientX - p.clientX, t.clientY - p.clientY) >
          TOUCH_SCROLL_SLOP_PX
      ) {
        // Moved before the hold: a scroll, not a selection.
        if (p.timer) clearTimeout(p.timer);
        pressRef.current = null;
      }
    };
    svg.addEventListener("touchmove", onMove, { passive: false });
    return () => svg.removeEventListener("touchmove", onMove);
  }, [webtoonTouch, enabled]);
  React.useEffect(() => clearPress, []);
  // A webtoon press-and-hold still waiting must not arm after the mode
  // ended (Esc, the pill's Cancel).
  React.useEffect(() => {
    if (!enabled) clearPress();
  }, [enabled]);

  function handleTouchStart(e: React.TouchEvent<SVGSVGElement>) {
    if (!enabled || !imgRef.current) return;
    if (e.touches.length !== 1) {
      clearPress();
      setDrag(null);
      return;
    }
    if (captureRef.current) return;
    const t = e.touches[0]!;
    const { x, y } = clientToImagePercent(t.clientX, t.clientY);
    if (!webtoonTouch) {
      setDrag({ startX: x, startY: y, currentX: x, currentY: y });
      return;
    }
    clearPress();
    const press = {
      x,
      y,
      clientX: t.clientX,
      clientY: t.clientY,
      armed: false,
      timer: null as ReturnType<typeof setTimeout> | null,
    };
    press.timer = setTimeout(() => {
      press.timer = null;
      if (pressRef.current !== press) return;
      press.armed = true;
      setDrag({ startX: x, startY: y, currentX: x, currentY: y });
    }, TOUCH_HOLD_MS);
    pressRef.current = press;
  }

  function handleTouchMove(e: React.TouchEvent<SVGSVGElement>) {
    if (!drag || !imgRef.current) return;
    const t = e.touches[0];
    if (!t) return;
    const { x, y } = clientToImagePercent(t.clientX, t.clientY);
    setDrag((prev) => (prev ? { ...prev, currentX: x, currentY: y } : prev));
  }

  function handleTouchEnd() {
    const press = pressRef.current;
    clearPress();
    if (drag) {
      const release = drag;
      setDrag(null);
      void completeDrag(release);
      return;
    }
    // Webtoon quick tap: released before the hold armed, without moving.
    if (press && !press.armed) {
      void completeDrag({
        startX: press.x,
        startY: press.y,
        currentX: press.x,
        currentY: press.y,
      });
    }
  }

  function handleTouchCancel() {
    clearPress();
    setDrag(null);
  }

  /** A capture may start or finish only in text capture with no editor
   *  open. Read from the store, not this render: a toast's Try again runs
   *  from an old closure. */
  function captureAllowed(): boolean {
    const st = useReaderStore.getState();
    return st.markerMode === "select-text" && st.pendingMarker === null;
  }

  function showFailure(message: string, retry?: () => void) {
    if (failureToastRef.current != null) toast.dismiss(failureToastRef.current);
    failureToastRef.current = toast.error(
      message,
      retry ? { action: { label: "Try again", onClick: retry } } : undefined,
    );
  }

  /** Run one capture: nothing if another is in flight; open the editor on
   *  what it produced unless it was cancelled meanwhile. */
  async function runCapture(
    box: TextRegionView | null,
    work: (isCurrent: () => boolean) => Promise<PendingMarker | null>,
  ) {
    if (captureRef.current || !captureAllowed()) return;
    const token = Symbol("capture");
    captureRef.current = token;
    const isCurrent = () => captureRef.current === token;
    setBusyRegion(box);
    captureToastRef.current = toast.loading("Reading text…");
    try {
      const pending = await work(isCurrent);
      // Still capturing, still in the mode, and no editor opened meanwhile
      // (a pending marker holds unsaved edits that must not be replaced).
      if (pending && isCurrent() && captureAllowed()) beginMarkerEdit(pending);
    } finally {
      if (isCurrent()) {
        if (captureToastRef.current != null) {
          toast.dismiss(captureToastRef.current);
        }
        captureToastRef.current = null;
        captureRef.current = null;
        setBusyRegion(null);
      }
    }
  }

  // Tap / Enter on a detected box. Shared by the pointer/touch tap path
  // and the keyboard proxies: the region IS the detector's box, so the
  // request runs recognizer-only and goes through the same query the
  // page-text panel uses for that box (one cache entry, client and
  // server). A failure leaves the user in the mode with the reason; a
  // transient one offers Try again.
  async function captureTextBox(box: TextRegionView) {
    if (!captureAllowed()) return;
    // The size is only for the pixel fallback; the box goes as percent.
    const size = pageSizeNow();
    await runCapture(box, async (isCurrent) => {
      const region: MarkerRegion = {
        x: box.x,
        y: box.y,
        w: box.w,
        h: box.h,
        shape: "text",
      };
      let ocr: OcrResult | null;
      try {
        const { boxTextQueryOptions } = await import("./marker-selection");
        const options = boxTextQueryOptions(issueId, pageIndex, box, size);
        try {
          ocr = await queryClient.fetchQuery(options);
        } catch (err) {
          // The page-text panel was closed while this tap shared its
          // in-flight read: the query lost its last observer and was
          // cancelled. Not a failure; ask once more.
          if (!isCancelledError(err)) throw err;
          ocr = await queryClient.fetchQuery(options);
        }
      } catch (err) {
        if (!isCurrent() || isCancelledError(err)) return null;
        const failure = toOcrRequestError(err);
        showFailure(
          `Couldn't read the text. ${failure.message}`,
          failure.transient ? () => void captureTextBox(box) : undefined,
        );
        return null;
      }
      if (!isCurrent()) return null;
      const base = newHighlight(pageIndex, region, true);
      if (!ocr) {
        toast.message(NO_TEXT_MESSAGE);
        return base;
      }
      return {
        ...base,
        selection: { text: ocr.text, ocr_confidence: ocr.confidence },
      };
    });
  }

  // Release path shared by both event bindings: tap-to-capture on a
  // near-zero drag, otherwise finalize the dragged region into a
  // PendingMarker and open the editor.
  async function completeDrag(release: DragState) {
    if (captureRef.current) return;
    const travel = Math.hypot(
      release.currentX - release.startX,
      release.currentY - release.startY,
    );
    if (travel < TAP_TRAVEL_MAX && textBoxes.length > 0) {
      const hit = hitTestBox(textBoxes, release.currentX, release.currentY);
      if (hit) {
        await captureTextBox(hit);
        return;
      }
    }

    const region = dragToRegion(release, markerModeShape(markerMode));
    if (!region) {
      // A tap that hit no box (or landed before the outlines loaded) in
      // text capture: stay in the mode, nothing to do. Other modes keep
      // the old "a tap leaves the mode" exit.
      if (markerMode !== "select-text") setMarkerMode("idle");
      return;
    }

    if (markerMode === "select-text") {
      // The drawn rectangle goes as percent; a known size only adds the
      // pixel fallback for an older server.
      const size = pageSizeNow();
      // Snap-to-box only when the page's detect cache is known-warm (the
      // regions fetch succeeded) — never risk a cold detector run here.
      const detect = regionsQuery.isSuccess;
      await runCapture(null, async (isCurrent) => {
        const base = newHighlight(pageIndex, region, false);
        let ocr: OcrResult | null;
        try {
          const { ocrCroppedRegion } = await import("./marker-selection");
          ocr = await ocrCroppedRegion(
            { issueId, pageIndex, region, pageSize: size },
            { detect },
          );
        } catch (err) {
          if (!isCurrent()) return null;
          const failure = toOcrRequestError(err);
          showFailure(
            `Couldn't read the text. ${failure.message}${
              failure.transient
                ? " Use Detect text in the editor to try again."
                : ""
            }`,
          );
          return base;
        }
        if (!isCurrent()) return null;
        if (!ocr) {
          toast.message(NO_TEXT_MESSAGE);
          return base;
        }
        // Snap the new marker to the detector's box when one came back:
        // the saved region hugs the text instead of the rough drag. The
        // box is in the pixels of the page the server decoded, so it is
        // converted with the size the response reports; the client's own
        // size only stands in for a server that does not report it.
        const snapSize = ocr.pageSize ?? size;
        const snapped =
          ocr.refinedBbox && snapSize
            ? {
                ...pagePixelsToRegion(ocr.refinedBbox, snapSize),
                shape: region.shape,
              }
            : null;
        return {
          ...base,
          region: snapped ?? region,
          regionFitted: !!snapped,
          selection: { text: ocr.text, ocr_confidence: ocr.confidence },
        };
      });
      return;
    }

    if (markerMode === "select-image") {
      const base = newHighlight(pageIndex, region, false);
      try {
        const { sha256CroppedRegion } = await import("./marker-selection");
        const hash = await sha256CroppedRegion({
          issueId,
          pageIndex,
          region,
        });
        beginMarkerEdit(
          hash ? { ...base, selection: { image_hash: hash } } : base,
        );
      } catch (err) {
        console.warn("markers: image hash failed", err);
        beginMarkerEdit(base);
      }
      return;
    }

    beginMarkerEdit(newHighlight(pageIndex, region, false));
  }

  const cursorClass =
    markerMode === "idle"
      ? "pointer-events-none"
      : hoverRegion
        ? "cursor-pointer pointer-events-auto"
        : "cursor-crosshair pointer-events-auto";

  // SVG sits inside the wrapper but is sized/positioned to overlay the
  // rendered image exactly. Until the first measurement lands we
  // fall back to `inset-0` so legacy markers still render on the
  // initial paint (their old wrapper-relative coords align with the
  // wrapper bounds at that point).
  const svgPositionStyle: React.CSSProperties = imgRect
    ? {
        top: imgRect.top,
        left: imgRect.left,
        width: imgRect.width,
        height: imgRect.height,
      }
    : { top: 0, left: 0, right: 0, bottom: 0 };
  return (
    <TooltipProvider delayDuration={250}>
      <svg
        ref={svgRef}
        aria-hidden={markerMode === "idle" ? "true" : undefined}
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        style={{
          position: "absolute",
          ...svgPositionStyle,
          // While a select mode is live in a paged view the SVG owns the
          // drag outright: opting out of native touch handling keeps the
          // reader's pan-y / pinch-zoom from cancelling a selection
          // mid-drag (the touch path has no pointer capture at all, and
          // the pointer path otherwise loses near-vertical drags to native
          // scroll via pointercancel).
          // Webtoon is one long scroll, so there the page keeps vertical
          // panning and pinch; a touch drag starts after a press-and-hold
          // instead (see the touch handlers). The cost: on a touch screen
          // whose primary pointer is fine (a touch laptop), a touch drag
          // in webtoon scrolls instead of selecting — use the mouse there.
          touchAction: enabled
            ? viewMode === "webtoon"
              ? "pan-y pinch-zoom"
              : "none"
            : undefined,
        }}
        className={cn("z-20 select-none", cursorClass)}
        {...(touchInput
          ? {
              onTouchStart: handleTouchStart,
              onTouchMove: handleTouchMove,
              onTouchEnd: handleTouchEnd,
              onTouchCancel: handleTouchCancel,
            }
          : {
              onPointerDown: handlePointerDown,
              onPointerMove: handlePointerMove,
              onPointerUp: handlePointerUp,
              onPointerCancel: () => setDrag(null),
            })}
      >
        {regionMarkers.map((marker) => (
          <RegionMarkerRect
            key={marker.id}
            marker={marker}
            interactive={!enabled}
            onEdit={() => editExistingMarker(marker)}
          />
        ))}
        {textBoxes.map((region, i) => (
          <TextRegionOutline
            key={`text-region-${i}`}
            region={region}
            hovered={region === hoverRegion}
            busy={region === busyRegion}
          />
        ))}
        {drag ? <DragPreview drag={drag} mode={markerMode} /> : null}
      </svg>

      {/* Keyboard / screen-reader proxies for region markers (audit E4).
          The rects above live in an `aria-hidden` SVG and are only
          reachable with a pointer; these transparent buttons overlay each
          region so Tab/Enter reaches it and opens the editor (which owns
          the Delete affordance). Rendered only at rest — during a drag-
          select the SVG owns all pointer input, so the proxies stand
          down to avoid intercepting the new-marker drag. */}
      {!enabled && regionMarkers.length > 0 ? (
        <div
          className="pointer-events-none absolute"
          style={{ position: "absolute", ...svgPositionStyle }}
        >
          {regionMarkers.map((marker) => {
            const r = marker.region;
            if (!r) return null;
            return (
              <button
                key={marker.id}
                type="button"
                onClick={() => editExistingMarker(marker)}
                aria-label={regionMarkerLabel(marker)}
                className="pointer-events-auto absolute cursor-pointer rounded-sm focus-visible:ring-2 focus-visible:ring-white focus-visible:ring-offset-1 focus-visible:outline-none"
                style={{
                  left: `${r.x}%`,
                  top: `${r.y}%`,
                  width: `${r.w}%`,
                  height: `${r.h}%`,
                }}
              />
            );
          })}
        </div>
      ) : null}

      {/* Keyboard / screen-reader proxies for detected text regions
          (WP-4.8, audit AC-3). The bubble outlines are `pointer-events:
          none` rects in the SVG — reachable only by pointer. These
          buttons sit over each bubble in reading order so Tab reaches
          it and Enter / Space runs the same tap-to-OCR capture. They
          stay `pointer-events-none` themselves: the SVG must keep sole
          ownership of pointer input so drag-select still works, and
          keyboard activation doesn't need hit-testing. */}
      {markerMode === "select-text" && textBoxes.length > 0 ? (
        <div
          role="group"
          aria-label={`Detected text on page ${pageIndex + 1}`}
          className="pointer-events-none absolute z-20"
          style={{ position: "absolute", ...svgPositionStyle }}
        >
          {textBoxes.map((region, i) => (
            <button
              key={`text-region-proxy-${i}`}
              type="button"
              aria-label={`Capture text region ${i + 1} of ${textBoxes.length}`}
              aria-busy={region === busyRegion || undefined}
              onClick={() => {
                // One capture at a time (`runCapture`); the button is not
                // `disabled` — that would drop keyboard focus mid-OCR.
                void captureTextBox(region);
              }}
              onFocus={() => setHoverRegion(region)}
              onBlur={() => setHoverRegion(null)}
              className="focus-visible:ring-ring pointer-events-none absolute rounded-sm focus-visible:ring-2 focus-visible:ring-offset-1 focus-visible:outline-none"
              style={{
                left: `${region.x}%`,
                top: `${region.y}%`,
                width: `${region.w}%`,
                height: `${region.h}%`,
              }}
            />
          ))}
        </div>
      ) : null}

      {markerMode === "select-text" && pageVisible ? (
        <DetectionStatus
          pageNumber={pageIndex + 1}
          loading={
            detectEnabled && regionsQuery.isFetching && !regionsQuery.data
          }
          error={regionsQuery.isError ? regionsQuery.error : null}
          boxCount={regionsQuery.data ? textBoxes.length : null}
          onRetry={() => void regionsQuery.refetch()}
        />
      ) : null}

      {pagePins.length > 0 ? (
        <div
          className={cn(
            "pointer-events-none absolute right-2 z-30 flex flex-col items-end gap-1.5 transition-[top] duration-300 ease-out motion-reduce:transition-none",
            chromeShowing ? "top-14" : "top-2",
          )}
        >
          {pagePins.map((marker) => (
            <PagePin
              key={marker.id}
              marker={marker}
              issueId={issueId}
              onEdit={() =>
                beginMarkerEdit(
                  {
                    kind: marker.kind,
                    page_index: marker.page_index,
                    region: null,
                    selection: marker.selection ?? null,
                    body: marker.body ?? "",
                    is_favorite: marker.is_favorite,
                    tags: marker.tags,
                  },
                  marker.id,
                )
              }
            />
          ))}
        </div>
      ) : null}
    </TooltipProvider>
  );
}

/** Screen-reader label for a region marker's focusable proxy. Leads with
 *  the kind so the announcement is useful even when the note is empty. */
function regionMarkerLabel(marker: MarkerView): string {
  const kind = marker.kind.charAt(0).toUpperCase() + marker.kind.slice(1);
  const text = marker.selection?.text?.replace(/\s+/g, " ").trim();
  const body = marker.body?.replace(/\s+/g, " ").trim();
  const parts = [text ? `“${text}”` : "", body].filter(Boolean);
  const label = parts.length
    ? `${kind}: ${parts.join(" — ")}`
    : `${kind} region`;
  // The whole name is capped (captured text and caption alike): a screen
  // reader reads it in one breath.
  return label.length > REGION_LABEL_MAX
    ? `${label.slice(0, REGION_LABEL_MAX - 1).trimEnd()}…`
    : label;
}

type DragState = {
  startX: number;
  startY: number;
  currentX: number;
  currentY: number;
};

function dragToRegion(
  d: DragState,
  shape: MarkerRegion["shape"],
): MarkerRegion | null {
  const x = Math.min(d.startX, d.currentX);
  const y = Math.min(d.startY, d.currentY);
  const w = Math.abs(d.currentX - d.startX);
  const h = Math.abs(d.currentY - d.startY);
  if (w < 1 || h < 1) return null;
  return { x, y, w, h, shape };
}

function markerModeShape(mode: MarkerMode): MarkerRegion["shape"] {
  switch (mode) {
    case "select-text":
      return "text";
    case "select-image":
      return "image";
    case "select-rect":
    default:
      return "rect";
  }
}

/** A new highlight on `region`, without a selection yet. */
function newHighlight(
  pageIndex: number,
  region: MarkerRegion,
  regionFitted: boolean,
): PendingMarker {
  return {
    kind: "highlight",
    page_index: pageIndex,
    region,
    selection: null,
    body: "",
    is_favorite: false,
    tags: [],
    ...(regionFitted ? { regionFitted: true } : {}),
  };
}

/** Text-capture detection state for one page, as a live region: finding
 *  the boxes, why finding them failed (with Try again when that can help),
 *  that there are none, or (to screen readers only) how many there are.
 *  Drag-to-select works throughout; this only explains the outlines. */
function DetectionStatus({
  pageNumber,
  loading,
  error,
  boxCount,
  onRetry,
}: {
  pageNumber: number;
  loading: boolean;
  error: unknown;
  boxCount: number | null;
  onRetry: () => void;
}) {
  // A live region announces changes, not its first content: for a page
  // whose detection is already cached it would mount filled and say
  // nothing. Mount it empty and fill it a moment later.
  const [ready, setReady] = React.useState(false);
  React.useEffect(() => {
    const t = setTimeout(() => setReady(true), LIVE_REGION_FILL_DELAY_MS);
    return () => clearTimeout(t);
  }, []);
  const failure = error ? toOcrRequestError(error) : null;
  const pill =
    "bg-background/90 text-foreground absolute top-3 left-1/2 z-30 flex max-w-[calc(100%-1.5rem)] -translate-x-1/2 items-center gap-2 rounded-full px-3 py-1 text-xs shadow-md backdrop-blur";
  return (
    <div role="status" aria-live="polite">
      {!ready ? null : loading ? (
        <div className={cn(pill, "text-muted-foreground pointer-events-none")}>
          <Loader2 className="size-3 animate-spin" aria-hidden="true" />
          Finding text regions…
        </div>
      ) : failure ? (
        <div className={cn(pill, "pointer-events-auto rounded-2xl py-1.5")}>
          <span>
            {`Couldn't find the text on page ${pageNumber}. ${failure.message} You can still drag around text.`}
          </span>
          {failure.transient ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="shrink-0"
              onClick={onRetry}
            >
              <RotateCcw className="size-3.5" aria-hidden="true" />
              Try again
            </Button>
          ) : null}
        </div>
      ) : boxCount === 0 ? (
        <div className={cn(pill, "text-muted-foreground pointer-events-none")}>
          {`No text found on page ${pageNumber}. Drag around text to capture it.`}
        </div>
      ) : boxCount != null ? (
        <span className="sr-only">
          {`${boxCount} text ${boxCount === 1 ? "region" : "regions"} found. Tap one, or Tab to it and press Enter, to capture its text.`}
        </span>
      ) : null}
    </div>
  );
}

function RegionMarkerRect({
  marker,
  interactive,
  onEdit,
}: {
  marker: MarkerView;
  interactive: boolean;
  onEdit: () => void;
}) {
  const region = marker.region;
  if (!region) return null;
  // `vectorEffect="non-scaling-stroke"` keeps the rect outline a
  // uniform pixel width even though the parent SVG is stretched
  // non-uniformly via `viewBox` + `preserveAspectRatio="none"`. Without
  // it, the horizontal edges scaled along the page-height axis and
  // looked noticeably thicker than the vertical edges. 2px reads as a
  // proper "highlighted" border without dominating the artwork.
  return (
    <g
      onClick={(e) => {
        if (!interactive) return;
        e.stopPropagation();
        onEdit();
      }}
      style={
        interactive ? { pointerEvents: "auto", cursor: "pointer" } : undefined
      }
    >
      <rect
        x={region.x}
        y={region.y}
        width={region.w}
        height={region.h}
        fill={KIND_FILL[marker.kind]}
        stroke={KIND_STROKE[marker.kind]}
        strokeWidth={2}
        vectorEffect="non-scaling-stroke"
        strokeLinejoin="miter"
      />
    </g>
  );
}

function PagePin({
  marker,
  issueId,
  onEdit,
}: {
  marker: MarkerView;
  issueId: string;
  onEdit: () => void;
}) {
  const del = useDeleteMarker(marker.id, issueId, { silent: true });
  const restore = useRestoreMarkers();
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onEdit}
          className={cn(
            "pointer-events-auto inline-flex h-7 w-7 items-center justify-center rounded-full shadow-md ring-2 ring-white/30 backdrop-blur transition-transform hover:scale-110",
            KIND_PIN_BG[marker.kind],
          )}
          aria-label={`${marker.kind} on this page`}
        >
          <KindIcon kind={marker.kind} />
        </button>
      </TooltipTrigger>
      <TooltipContent side="left">
        <div className="space-y-1">
          <div className="text-xs font-medium capitalize">{marker.kind}</div>
          {marker.body ? (
            <div className="text-muted-foreground line-clamp-3 max-w-[18rem] text-xs">
              {marker.body}
            </div>
          ) : null}
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              const snapshot = marker;
              del.mutate(undefined, {
                onSuccess: () =>
                  showUndoToast({
                    message: "Removed",
                    onUndo: () => restore.mutateAsync([snapshot]),
                  }),
              });
            }}
            className="text-destructive hover:text-destructive/80 inline-flex items-center gap-1 text-xs"
          >
            <Trash2 className="size-3" /> Remove
          </button>
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

/** Detected-bubble outline shown in text-capture mode. Subtle at
 *  rest (the artwork stays readable), brightened + lightly filled on
 *  hover, pulsing while its OCR is in flight. `pointer-events: none`
 *  throughout — the parent SVG owns all pointer handling so the
 *  drag path is never obstructed. Reuses the select-text drag
 *  preview's blue so the affordances read as one family. */
function TextRegionOutline({
  region,
  hovered,
  busy,
}: {
  region: TextRegionView;
  hovered: boolean;
  busy: boolean;
}) {
  const active = hovered || busy;
  return (
    <rect
      x={region.x}
      y={region.y}
      width={region.w}
      height={region.h}
      fill={active ? "rgba(59, 130, 246, 0.10)" : "transparent"}
      stroke="rgb(59, 130, 246)"
      strokeOpacity={active ? 1 : 0.55}
      strokeWidth={active ? 2 : 1.5}
      vectorEffect="non-scaling-stroke"
      strokeLinejoin="miter"
      strokeDasharray="4 3"
      pointerEvents="none"
      className={busy ? "animate-pulse" : undefined}
    />
  );
}

function DragPreview({ drag, mode }: { drag: DragState; mode: MarkerMode }) {
  const x = Math.min(drag.startX, drag.currentX);
  const y = Math.min(drag.startY, drag.currentY);
  const w = Math.abs(drag.currentX - drag.startX);
  const h = Math.abs(drag.currentY - drag.startY);
  const stroke =
    mode === "select-text"
      ? "rgb(59, 130, 246)"
      : mode === "select-image"
        ? "rgb(168, 85, 247)"
        : "rgb(234, 179, 8)";
  // Match the committed-rect treatment: non-scaling stroke at the same
  // 2px width so the live drag preview and the saved marker look
  // identical. Dash spacing is in CSS pixels (since the stroke is no
  // longer in user units, dasharray needs to be too).
  return (
    <rect
      x={x}
      y={y}
      width={w}
      height={h}
      fill="rgba(255, 255, 255, 0.10)"
      stroke={stroke}
      strokeWidth={2}
      vectorEffect="non-scaling-stroke"
      strokeLinejoin="miter"
      strokeDasharray="6 4"
      pointerEvents="none"
    />
  );
}

function KindIcon({ kind }: { kind: MarkerKind }) {
  switch (kind) {
    case "bookmark":
      return <BookmarkCheck className="size-4" aria-hidden="true" />;
    case "note":
      return <MessageSquareText className="size-4" aria-hidden="true" />;
    case "highlight":
      return <Star className="size-4" aria-hidden="true" />;
  }
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
