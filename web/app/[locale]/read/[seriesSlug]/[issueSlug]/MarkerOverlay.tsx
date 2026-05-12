"use client";

import * as React from "react";
import { BookmarkCheck, MessageSquareText, Star, Trash2 } from "lucide-react";
import { toast } from "sonner";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useIssueMarkers } from "@/lib/api/queries";
import { useDeleteMarker } from "@/lib/api/mutations";
import {
  useReaderStore,
  type MarkerMode,
  type PendingMarker,
} from "@/lib/reader/store";
import { cn } from "@/lib/utils";
import type { MarkerKind, MarkerRegion, MarkerView } from "@/lib/api/types";

import { ocrCroppedRegion, sha256CroppedRegion } from "./marker-selection";

/** Per-kind colors for the SVG rect markers. Translucent fill + solid
 *  stroke so highlights remain readable over the page content
 *  underneath. */
const KIND_FILL: Record<MarkerKind, string> = {
  bookmark: "rgba(245, 158, 11, 0.18)",
  note: "rgba(59, 130, 246, 0.18)",
  highlight: "rgba(234, 179, 8, 0.22)",
};
const KIND_STROKE: Record<MarkerKind, string> = {
  bookmark: "rgb(245, 158, 11)",
  note: "rgb(59, 130, 246)",
  highlight: "rgb(234, 179, 8)",
};

const KIND_PIN_BG: Record<MarkerKind, string> = {
  bookmark: "bg-amber-500/90 text-white",
  note: "bg-blue-500/90 text-white",
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
  wrapperRef,
  imgRef,
  naturalSize,
}: {
  issueId: string;
  pageIndex: number;
  /** The wrapper element. SVG is positioned absolutely INSIDE this
   *  element, but offset/sized to match the image (not the wrapper)
   *  so coord percentages are image-relative. Critical when the
   *  wrapper is wider than the rendered image — e.g. fit=height with
   *  a tall comic page centered in a wide viewport. */
  wrapperRef: React.RefObject<HTMLElement | null>;
  /** The actually-rendered `<img>` element. Used to compute the SVG's
   *  position + dimensions so it overlays the image exactly. Pointer
   *  coordinates are taken relative to this image's bounds. */
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
  React.useLayoutEffect(() => {
    const wrap = wrapperRef.current;
    const img = imgRef.current;
    if (!wrap || !img) {
      // No image yet (page just remounted) — clear the stale rect so
      // the SVG falls back to `inset-0` until the new image lays out,
      // instead of pinning the overlay at the old issue's bounds.
      setImgRect(null);
      return;
    }
    // Clear immediately on rebind so a stale rect from the previous
    // issue can't flash through during the (sub-frame) window before
    // `recompute()` writes the new value. Setting to null is cheap;
    // the SVG's inset-0 fallback covers the wrapper for that brief
    // moment, and that's only visible if measurement returns 0×0
    // (image still loading).
    setImgRect(null);
    const recompute = () => {
      const wr = wrap.getBoundingClientRect();
      const ir = img.getBoundingClientRect();
      // Skip while either element hasn't laid out yet — emitting
      // zeros here would render the overlay as a 0×0 box and the
      // first real measurement on the next observer tick would
      // create a visible jump.
      if (ir.width <= 0 || ir.height <= 0) return;
      setImgRect({
        top: ir.top - wr.top,
        left: ir.left - wr.left,
        width: ir.width,
        height: ir.height,
      });
    };
    recompute();
    const ro = new ResizeObserver(() => recompute());
    ro.observe(wrap);
    ro.observe(img);
    // Browsers don't fire ResizeObserver for late-arriving image
    // bytes that change the `<img>` element's intrinsic size, so
    // we also hook the load event explicitly.
    img.addEventListener("load", recompute);
    return () => {
      ro.disconnect();
      img.removeEventListener("load", recompute);
    };
    // pageIndex/issueId are the implicit identity of the rendered
    // <img> — when they change, PageImage remounts a new element and
    // we must rebind the observer to it.
  }, [wrapperRef, imgRef, pageIndex, issueId]);

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
    if (!drag || !imgRef.current) return;
    e.preventDefault();
    const rect = imgRef.current.getBoundingClientRect();
    const x = clamp(((e.clientX - rect.left) / rect.width) * 100, 0, 100);
    const y = clamp(((e.clientY - rect.top) / rect.height) * 100, 0, 100);
    setDrag((prev) => (prev ? { ...prev, currentX: x, currentY: y } : prev));
  }

  async function handlePointerUp(e: React.PointerEvent<SVGSVGElement>) {
    if (!drag) return;
    e.preventDefault();
    (e.target as SVGSVGElement).releasePointerCapture(e.pointerId);
    const region = dragToRegion(drag, markerModeShape(markerMode));
    setDrag(null);
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
    );
    beginMarkerEdit(pending);
  }

  const cursorClass =
    markerMode === "idle"
      ? "pointer-events-none"
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
            onEdit={() =>
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
              )
            }
          />
        ))}
        {drag ? <DragPreview drag={drag} mode={markerMode} /> : null}
      </svg>

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
      const ocr = await ocrCroppedRegion({
        issueId,
        pageIndex,
        region,
        naturalSize,
      });
      toast.dismiss(ocrToast);
      if (ocr && ocr.text.trim()) {
        return {
          ...base,
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
  const del = useDeleteMarker(marker.id, issueId);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onEdit}
          className={cn(
            "pointer-events-auto inline-flex h-7 w-7 items-center justify-center rounded-full ring-2 shadow-md ring-white/30 backdrop-blur transition-transform hover:scale-110",
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
              del.mutate();
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
