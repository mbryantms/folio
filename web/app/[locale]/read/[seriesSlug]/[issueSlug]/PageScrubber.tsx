"use client";

import { useRef, useState } from "react";

import type { Direction } from "@/lib/reader/detect";
import {
  scrubFraction,
  scrubKeyTarget,
  scrubPageAt,
  scrubPageX,
  scrubPreviewLeft,
} from "@/lib/reader/scrub";
import { useReaderStore } from "@/lib/reader/store";
import { SWIPE_IGNORE_ATTR } from "@/lib/reader/use-swipe";
import { pageThumbUrl, withContentVersion } from "@/lib/urls";

/** Preview card width (px): the thumbnail plus its padding. */
const CARD_W = 92;
/** A touch has to travel this far before the lift jumps — a plain tap
 *  under the chrome is far more likely aimed at the page than the bar. */
const TOUCH_SLOP_PX = 6;

type Preview = { page: number; left: number };

/**
 * Page scrubber along the bottom edge of the reader chrome, over the
 * thin reading-progress bar (`ReadingProgress`). Drag it — or click it —
 * to jump to a page; while dragging (and while a mouse hovers it) a
 * floating card under the bar shows that page's thumbnail and "p / N",
 * following the knob. The page changes on release, not live, so a drag
 * across a 200-page issue doesn't fetch 200 full pages.
 *
 * Thumbnails are the page strip's `?variant=strip` images (same URLs as
 * `PageStrip` and the `PageImage` blur-up, so nearby pages are cached).
 * RTL runs the bar right to left. A keyboard `role="slider"`: arrows,
 * PageUp / PageDown, Home / End jump at once (`scrubKeyTarget`); the
 * reader keymap leaves arrow keys inside a slider alone (keybinds.ts).
 *
 * The hit area straddles the header's bottom edge — 8px inside (the
 * header's bottom padding, clear of its buttons) and the rest below —
 * taller on touch (24px / 30px, above the WCAG 2.2 target-size floor). It opts out of the reader's page-turn drag
 * (`SWIPE_IGNORE_ATTR`). On touch the header buttons' 44px tap areas
 * (`folio-tap`) reach 4px into that 8px band, and this bar, painted above
 * them, would take all 4px; it starts 2px lower instead (30px tall, rail
 * and knob held in place) so the overlap is split evenly.
 */
export function PageScrubber({
  issueId,
  totalPages,
  currentPage,
  visiblePages,
  direction,
  urlVersion,
}: {
  issueId: string;
  totalPages: number;
  currentPage: number;
  /** Pages on screen in double view (keyboard steps leave the spread). */
  visiblePages?: readonly number[];
  direction: Direction;
  urlVersion: string | null;
}) {
  const setPage = useReaderStore((s) => s.setPage);
  const barRef = useRef<HTMLDivElement>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<{
    id: number;
    x0: number;
    touch: boolean;
    moved: boolean;
    page: number;
  } | null>(null);

  if (totalPages <= 1) return null;

  const pageAt = (clientX: number): Preview | null => {
    const bar = barRef.current;
    if (!bar) return null;
    const rect = bar.getBoundingClientRect();
    const page = scrubPageAt(clientX, rect, totalPages, direction);
    const x = scrubPageX(page, totalPages, rect.width, direction);
    return { page, left: scrubPreviewLeft(x, CARD_W, rect.width) };
  };

  const shown = preview?.page ?? currentPage;
  const f = scrubFraction(shown, totalPages);
  const knobPct = (direction === "rtl" ? 1 - f : f) * 100;
  const span = {
    first: visiblePages?.length ? Math.min(...visiblePages) : currentPage,
    last: visiblePages?.length ? Math.max(...visiblePages) : currentPage,
  };

  return (
    <div
      ref={barRef}
      role="slider"
      tabIndex={0}
      aria-label="Page"
      aria-orientation="horizontal"
      aria-valuemin={1}
      aria-valuemax={totalPages}
      aria-valuenow={currentPage + 1}
      aria-valuetext={`Page ${currentPage + 1} of ${totalPages}`}
      data-testid="page-scrubber"
      data-dragging={dragging ? "" : undefined}
      {...{ [SWIPE_IGNORE_ATTR]: "" }}
      className="group absolute inset-x-0 -bottom-4 z-10 h-6 cursor-pointer touch-none select-none focus-visible:outline-none pointer-coarse:-bottom-6 pointer-coarse:h-7.5"
      onPointerDown={(e) => {
        if (e.pointerType === "mouse" && e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.focus({ preventScroll: true });
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          // Synthetic / already-released pointer: the drag still works
          // while the pointer stays over the bar.
        }
        const p = pageAt(e.clientX);
        dragRef.current = {
          id: e.pointerId,
          x0: e.clientX,
          touch: e.pointerType === "touch",
          moved: false,
          page: p?.page ?? currentPage,
        };
        setDragging(true);
        setPreview(p);
      }}
      onPointerMove={(e) => {
        const d = dragRef.current;
        if (d && e.pointerId === d.id) {
          if (Math.abs(e.clientX - d.x0) >= TOUCH_SLOP_PX) d.moved = true;
          const p = pageAt(e.clientX);
          if (p) d.page = p.page;
          setPreview(p);
        } else if (!d && e.pointerType === "mouse") {
          setPreview(pageAt(e.clientX));
        }
      }}
      onPointerUp={(e) => {
        const d = dragRef.current;
        if (!d || e.pointerId !== d.id) return;
        dragRef.current = null;
        setDragging(false);
        if (!d.touch || d.moved) setPage(d.page);
        // A mouse is still over the bar: keep previewing under it.
        if (e.pointerType !== "mouse") setPreview(null);
      }}
      onPointerCancel={() => {
        dragRef.current = null;
        setDragging(false);
        setPreview(null);
      }}
      onPointerLeave={(e) => {
        if (!dragRef.current && e.pointerType === "mouse") setPreview(null);
      }}
      onKeyDown={(e) => {
        const target = scrubKeyTarget(e.key, span, totalPages, direction);
        if (target === null) return;
        e.preventDefault();
        // The reader keymap would otherwise also see PageUp / PageDown.
        e.stopPropagation();
        setPage(target);
      }}
    >
      {/* Rail, drawn over the progress bar at the header's bottom edge
       * (8px down the hit area); thickens on hover / drag / focus. */}
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 top-1.5 h-0.5 bg-transparent transition-[height,background-color] duration-150 group-hover:h-1 group-hover:bg-white/15 group-focus-visible:h-1 group-focus-visible:bg-white/15 group-data-dragging:h-1 group-data-dragging:bg-white/15 motion-reduce:transition-none pointer-coarse:top-1"
      />
      <span
        aria-hidden="true"
        className="bg-accent pointer-events-none absolute top-2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full opacity-0 shadow ring-2 ring-neutral-950 transition-opacity duration-150 group-hover:opacity-100 group-focus-visible:opacity-100 group-focus-visible:ring-white group-data-dragging:opacity-100 motion-reduce:transition-none pointer-coarse:top-1.5"
        style={{ left: `${knobPct}%` }}
      />
      {preview ? (
        <span
          aria-hidden="true"
          data-testid="page-scrubber-preview"
          className="pointer-events-none absolute top-7 flex flex-col items-center gap-1 rounded-lg border border-neutral-700/80 bg-neutral-950/90 p-1.5 shadow-xl backdrop-blur pointer-coarse:top-8.5"
          style={{ left: preview.left, width: CARD_W }}
        >
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            key={preview.page}
            src={withContentVersion(
              `${pageThumbUrl(issueId, preview.page)}?variant=strip`,
              urlVersion,
            )}
            alt=""
            decoding="async"
            draggable={false}
            className="block h-30 w-20 rounded bg-neutral-900 object-cover"
          />
          <span className="text-xs text-neutral-200 tabular-nums">
            {preview.page + 1} / {totalPages}
          </span>
        </span>
      ) : null}
    </div>
  );
}
