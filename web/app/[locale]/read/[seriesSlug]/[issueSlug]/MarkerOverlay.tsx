"use client";

import * as React from "react";
import {
  BookmarkCheck,
  Loader2,
  MessageSquareText,
  Star,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useIssueMarkers, useIssuePageTextRegions } from "@/lib/api/queries";
import { useCreateMarker, useDeleteMarker } from "@/lib/api/mutations";
import { markerToCreateReq } from "@/lib/markers/recreate";
import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import {
  useReaderStore,
  type MarkerMode,
  type PendingMarker,
} from "@/lib/reader/store";
import { cn } from "@/lib/utils";
import type {
  MarkerKind,
  MarkerRegion,
  MarkerView,
  TextRegionView,
} from "@/lib/api/types";

/** Pointer travel (percent of image size) below which a release
 *  counts as a tap rather than a drag. ~0.8% of a 1000px-wide page
 *  is 8px — forgiving enough for touch, far below any real drag. */
const TAP_TRAVEL_MAX = 0.8;

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
  naturalSize,
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
  /** Image's natural pixel dimensions — fed to the OCR / image-hash
   *  paths so they sample the source image at native resolution. */
  naturalSize: { width: number; height: number } | null;
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
  const regionsQuery = useIssuePageTextRegions(
    issueId,
    pageIndex,
    markerMode === "select-text" && pageVisible,
  );
  const textRegions = React.useMemo(
    () =>
      markerMode === "select-text" ? (regionsQuery.data?.regions ?? []) : [],
    [markerMode, regionsQuery.data],
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

  function handlePointerDown(e: React.PointerEvent<SVGSVGElement>) {
    if (!enabled || !imgRef.current) return;
    e.preventDefault();
    // Coords are computed against the IMAGE's rect, not the wrapper's,
    // so a drag is stored in image-relative percentages regardless of
    // whether the wrapper has whitespace around the image (e.g. at
    // fit=height when the page is narrower than the viewport).
    const rect = imgRef.current.getBoundingClientRect();
    const x = clamp(((e.clientX - rect.left) / rect.width) * 100, 0, 100);
    const y = clamp(((e.clientY - rect.top) / rect.height) * 100, 0, 100);
    setDrag({ startX: x, startY: y, currentX: x, currentY: y });
    (e.target as SVGSVGElement).setPointerCapture(e.pointerId);
  }

  function handlePointerMove(e: React.PointerEvent<SVGSVGElement>) {
    if (!imgRef.current) return;
    const rect = imgRef.current.getBoundingClientRect();
    const x = clamp(((e.clientX - rect.left) / rect.width) * 100, 0, 100);
    const y = clamp(((e.clientY - rect.top) / rect.height) * 100, 0, 100);
    if (!drag) {
      // Idle hover in text mode: light up the bubble under the
      // pointer. The rects can't take CSS :hover themselves (they're
      // pointer-events: none so dragging stays unobstructed).
      if (textRegions.length > 0) {
        setHoverRegion(hitTestRegion(textRegions, x, y));
      }
      return;
    }
    e.preventDefault();
    setDrag((prev) => (prev ? { ...prev, currentX: x, currentY: y } : prev));
  }

  async function handlePointerUp(e: React.PointerEvent<SVGSVGElement>) {
    if (!drag) return;
    e.preventDefault();
    (e.target as SVGSVGElement).releasePointerCapture(e.pointerId);
    const release = drag;
    setDrag(null);

    // Tap-to-OCR: a near-zero drag that lands on a detected bubble
    // OCRs that bubble. The region IS the detector's bbox, so the
    // request runs recognizer-only (`detect: false`) and is fast.
    const travel = Math.hypot(
      release.currentX - release.startX,
      release.currentY - release.startY,
    );
    if (travel < TAP_TRAVEL_MAX && textRegions.length > 0) {
      const hit = hitTestRegion(
        textRegions,
        release.currentX,
        release.currentY,
      );
      if (hit) {
        const region: MarkerRegion = {
          x: hit.x,
          y: hit.y,
          w: hit.w,
          h: hit.h,
          shape: "text",
        };
        // The regions payload carries the decoded page dims, so a
        // tap works even before the <img> has reported naturalSize.
        const size =
          naturalSize ??
          (regionsQuery.data
            ? {
                width: regionsQuery.data.page_w,
                height: regionsQuery.data.page_h,
              }
            : null);
        setBusyRegion(hit);
        try {
          const pending = await finalizePending(
            "select-text",
            pageIndex,
            region,
            issueId,
            size,
            { detect: false },
          );
          beginMarkerEdit(pending);
        } finally {
          setBusyRegion(null);
        }
        return;
      }
    }

    const region = dragToRegion(release, markerModeShape(markerMode));
    if (!region) {
      setMarkerMode("idle");
      return;
    }
    const pending = await finalizePending(
      markerMode,
      pageIndex,
      region,
      issueId,
      naturalSize,
      {
        // Snap-to-bubble on manual drags only when the page's
        // detect cache is known-warm (the regions fetch succeeded) —
        // never risk a cold detector run from the drag path.
        detect: markerMode === "select-text" && regionsQuery.isSuccess,
      },
    );
    beginMarkerEdit(pending);
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
        aria-hidden={markerMode === "idle" ? "true" : undefined}
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        style={{ position: "absolute", ...svgPositionStyle }}
        className={cn("z-20 select-none", cursorClass)}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={() => setDrag(null)}
      >
        {regionMarkers.map((marker) => (
          <RegionMarkerRect
            key={marker.id}
            marker={marker}
            interactive={!enabled}
            onEdit={() => editExistingMarker(marker)}
          />
        ))}
        {textRegions.map((region, i) => (
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

      {markerMode === "select-text" && pageVisible && regionsQuery.isLoading ? (
        // Non-blocking detection progress: drag-to-select works the
        // whole time, the pill just explains why outlines haven't
        // appeared yet (a cold detector run takes seconds).
        <div className="bg-background/85 text-muted-foreground pointer-events-none absolute top-3 left-1/2 z-30 flex -translate-x-1/2 items-center gap-1.5 rounded-full px-3 py-1 text-xs shadow-md backdrop-blur">
          <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
          Finding text regions…
        </div>
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
  const body = marker.body?.trim();
  return body ? `${kind}: ${body}` : `${kind} region`;
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

/** Smallest detected region containing the point, or `null`. The
 *  smallest-wins rule disambiguates nested detections (the detector
 *  emits both block- and line-level boxes that can overlap). */
function hitTestRegion(
  regions: readonly TextRegionView[],
  x: number,
  y: number,
): TextRegionView | null {
  let best: TextRegionView | null = null;
  let bestArea = Infinity;
  for (const r of regions) {
    if (x < r.x || x > r.x + r.w || y < r.y || y > r.y + r.h) continue;
    const area = r.w * r.h;
    if (area < bestArea) {
      bestArea = area;
      best = r;
    }
  }
  return best;
}

/** Branch on selection mode to populate `selection` from the cropped
 *  pixels. Falls back to plain rect when OCR / hashing isn't
 *  available. OCR runs synchronously here (worst-case 2-4 seconds)
 *  with a toast so the user knows the editor will pop in a moment. */
async function finalizePending(
  mode: MarkerMode,
  pageIndex: number,
  region: MarkerRegion,
  issueId: string,
  naturalSize: { width: number; height: number } | null,
  opts: { detect?: boolean } = {},
): Promise<PendingMarker> {
  const base: PendingMarker = {
    kind: "highlight",
    page_index: pageIndex,
    region,
    selection: null,
    body: "",
    is_favorite: false,
    tags: [],
  };

  if (mode === "select-text" && naturalSize) {
    const ocrToast = toast.loading("Reading text…");
    try {
      // Lazy-load the canvas-crop + OCR path (audit G6) — only users who
      // actually capture text pull these bytes; they stay out of the
      // reader's first-load JS.
      const { ocrCroppedRegion } = await import("./marker-selection");
      const ocr = await ocrCroppedRegion(
        {
          issueId,
          pageIndex,
          region,
          naturalSize,
        },
        { detect: opts.detect },
      );
      toast.dismiss(ocrToast);
      if (ocr && ocr.text.trim()) {
        // Snap the new marker to the detector's bubble outline when
        // one came back — the saved region hugs the bubble instead
        // of the rough drag. New pending markers only; re-detect on
        // an existing marker never rewrites stored geometry.
        const snapped = ocr.refinedBbox
          ? {
              x: clamp((ocr.refinedBbox.x / naturalSize.width) * 100, 0, 100),
              y: clamp((ocr.refinedBbox.y / naturalSize.height) * 100, 0, 100),
              w: clamp((ocr.refinedBbox.w / naturalSize.width) * 100, 0, 100),
              h: clamp((ocr.refinedBbox.h / naturalSize.height) * 100, 0, 100),
              shape: region.shape,
            }
          : region;
        return {
          ...base,
          region: snapped,
          selection: { text: ocr.text, ocr_confidence: ocr.confidence },
        };
      }
      toast.message(
        "Couldn't read any text in that region — saved as a plain highlight.",
      );
    } catch (err) {
      toast.dismiss(ocrToast);
      console.warn("markers: OCR failed", err);
    }
    return base;
  }

  if (mode === "select-image" && naturalSize) {
    try {
      const { sha256CroppedRegion } = await import("./marker-selection");
      const hash = await sha256CroppedRegion({
        issueId,
        pageIndex,
        region,
        naturalSize,
      });
      if (hash) {
        return { ...base, selection: { image_hash: hash } };
      }
    } catch (err) {
      console.warn("markers: image hash failed", err);
    }
  }

  return base;
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
  const create = useCreateMarker();
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
                  toast.success("Removed", {
                    duration: UNDO_TOAST_DURATION_MS,
                    action: {
                      label: "Undo",
                      onClick: () => create.mutate(markerToCreateReq(snapshot)),
                    },
                  }),
              });
            }}
            className="text-destructive hover:text-destructive/80 inline-flex items-center gap-1 text-xs"
          >
            <Trash2 className="h-3 w-3" /> Remove
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
      return <BookmarkCheck className="h-4 w-4" aria-hidden="true" />;
    case "note":
      return <MessageSquareText className="h-4 w-4" aria-hidden="true" />;
    case "highlight":
      return <Star className="h-4 w-4" aria-hidden="true" />;
  }
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
