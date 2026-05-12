"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useReaderStore } from "@/lib/reader/store";
import { computeSpreadGroups, groupIndexForPage } from "@/lib/reader/spreads";
import { useIssueMarkers } from "@/lib/api/queries";
import type { Direction } from "@/lib/reader/detect";
import type { MarkerKind, PageInfo } from "@/lib/api/types";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/** "favorite" is a star flag on any kind, not a kind itself — but for
 *  the dot palette we treat it as a synthetic dot so users still see a
 *  red marker for starred pages. */
type DotKind = MarkerKind | "favorite";

/** Color tokens per marker kind for the per-thumbnail dot. Matches
 *  the overlay's stroke palette so the same visual ID carries across
 *  the page strip and the in-page rect. */
const KIND_DOT_BG: Record<DotKind, string> = {
  bookmark: "bg-amber-500",
  note: "bg-blue-500",
  favorite: "bg-red-500",
  highlight: "bg-yellow-500",
};
/** Render order so kinds always stack the same way regardless of
 *  insertion order. Bookmarks first (most common), highlights last. */
const KIND_DISPLAY_ORDER: readonly DotKind[] = [
  "bookmark",
  "note",
  "favorite",
  "highlight",
];

/**
 * Mini-map / page strip overlay (§7.3, spec Phase 3 line 1410).
 *
 * Fixed bottom strip of small page thumbnails; click jumps to that page.
 * Direction is honored by reversing the row order for RTL.
 *
 * M3 polish: always-mounted with `data-state` driving a slide/fade animation;
 * the active thumb is scaled up with a softer ring + underline; visible thumbs
 * carry a Tooltip with the page number and double-spread flag.
 */
export function PageStrip({
  issueId,
  totalPages,
  currentPage,
  direction,
  pages,
}: {
  issueId: string;
  totalPages: number;
  currentPage: number;
  direction: Direction;
  pages: PageInfo[];
}) {
  const setPage = useReaderStore((s) => s.setPage);
  const visible = useReaderStore((s) => s.pageStripVisible);
  const viewMode = useReaderStore((s) => s.viewMode);
  const coverSolo = useReaderStore((s) => s.coverSolo);
  // Marker dots per page — one set of kinds per page index. Empty
  // when the user has no markers on this issue yet. Same TanStack
  // Query cache the reader overlay reads from, so the strip refreshes
  // in lockstep with the overlay.
  const issueMarkers = useIssueMarkers(issueId);
  // When the user has hidden marker overlays globally, the strip
  // suppresses its dots too — otherwise a "clean read" mode would
  // still leak marker presence through the bottom rail.
  const markersHidden = useReaderStore((s) => s.markersHidden);
  const markerKindsByPage = useMemo(() => {
    const m = new Map<number, Set<DotKind>>();
    if (markersHidden) return m;
    for (const marker of issueMarkers.data?.items ?? []) {
      const set = m.get(marker.page_index) ?? new Set<DotKind>();
      set.add(marker.kind);
      // Stars draw a red pip on top of whatever kind dot already exists
      // so users can spot favorited pages at a glance.
      if (marker.is_favorite) set.add("favorite");
      m.set(marker.page_index, set);
    }
    return m;
  }, [issueMarkers.data, markersHidden]);
  const activeRef = useRef<HTMLButtonElement>(null);
  const stripRef = useRef<HTMLOListElement>(null);
  // Tracks whether we've already centered the active thumb for the current
  // open. The first scroll after the strip becomes visible runs synchronously
  // (no smooth-scroll animation), so the user sees the strip slide in already
  // positioned on the active page instead of mid-pan from page 1.
  const hasCenteredForOpenRef = useRef(false);
  const [visibleRange, setVisibleRange] = useState({ start: 0, end: 48 });

  // Per-thumb layout width including the gap. Singles render at w-24
  // (96px) + gap-3 (12px) = 108px; doubles widen to w-48 (192px) + gap-3
  // (12px) = 204px. We build a prefix-sum of these so the virtualizer can
  // bucket `scrollLeft` into a thumb index regardless of mixed widths.
  // The active li's `mx-5` (40px extra) is absorbed by the overscan.
  //
  // The strip renders `totalPages` thumbs (one per actual archive page).
  // ComicInfo's `<Pages>` array is optional metadata that some publishers
  // ship truncated — falling back to it for the offset table would shrink
  // `visibleRange` so much that `shouldLoad` would be false for every
  // thumb past the metadata end, leaving most of the strip blank. The
  // per-page `double_page` lookup still uses `pages` and defaults to a
  // single-width slot when the entry is missing.
  const prefixOffsets = useMemo(() => {
    const offsets = new Array<number>(totalPages + 1);
    offsets[0] = 0;
    for (let i = 0; i < totalPages; i += 1) {
      const w = pages[i]?.double_page === true ? 204 : 108;
      offsets[i + 1] = offsets[i]! + w;
    }
    return offsets;
  }, [pages, totalPages]);

  // Pages currently visible in the reader. In single/webtoon this is just
  // the current page; in double-page view it's the full spread group, so a
  // pair `[i, i+1]` lights up both thumbnails. Cover-solo and aspect-ratio
  // spreads (`double_page === true`) collapse back to a single active thumb
  // because `computeSpreadGroups` already emits them as solo groups.
  const activePages = useMemo<readonly number[]>(() => {
    if (viewMode !== "double") return [currentPage];
    const groups = computeSpreadGroups(pages, { coverSolo });
    const idx = groupIndexForPage(groups, currentPage);
    return groups[idx] ?? [currentPage];
  }, [viewMode, coverSolo, pages, currentPage]);

  const updateVisibleRange = useCallback(() => {
    const el = stripRef.current;
    if (!el) {
      return;
    }
    const overscan = 8;
    const left = Math.abs(el.scrollLeft);
    const right = left + el.clientWidth;
    // Visual order: in RTL the strip iterates indices reversed, so the
    // visible range here is in *visual* (row) order, not in page order.
    // The virtualizer consumes it via `visualIndex` later in render.
    const total = totalPages;
    let visualStart = 0;
    let visualEnd = total - 1;
    // For each item, compute its visual [start, end] x-offset along the
    // strip. RTL flips the offsets — index 0 sits at the far right.
    const itemEnd = (i: number) =>
      direction === "rtl"
        ? prefixOffsets[total]! - prefixOffsets[i]!
        : prefixOffsets[i + 1]!;
    const itemStart = (i: number) =>
      direction === "rtl"
        ? prefixOffsets[total]! - prefixOffsets[i + 1]!
        : prefixOffsets[i]!;
    for (let i = 0; i < total; i += 1) {
      if (itemEnd(i) >= left) {
        visualStart = i;
        break;
      }
    }
    for (let i = total - 1; i >= 0; i -= 1) {
      if (itemStart(i) <= right) {
        visualEnd = i;
        break;
      }
    }
    const start = Math.max(0, visualStart - overscan);
    const end = Math.min(total - 1, visualEnd + overscan);
    setVisibleRange((prev) =>
      prev.start === start && prev.end === end ? prev : { start, end },
    );
  }, [direction, totalPages, prefixOffsets]);

  // Center the active thumb when it changes, smoothly. The very first scroll
  // after the strip becomes visible runs instantly so the strip slides in
  // already-positioned instead of doing a long smooth-pan during the open
  // animation.
  useEffect(() => {
    if (!visible) {
      hasCenteredForOpenRef.current = false;
      return;
    }
    const el = activeRef.current;
    if (!el) return;
    const reduced =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    const isFirstScroll = !hasCenteredForOpenRef.current;
    el.scrollIntoView({
      block: "nearest",
      inline: "center",
      behavior: reduced || isFirstScroll ? "auto" : "smooth",
    });
    hasCenteredForOpenRef.current = true;
    requestAnimationFrame(updateVisibleRange);
  }, [currentPage, updateVisibleRange, visible]);

  useEffect(() => {
    if (!visible) return;
    updateVisibleRange();
  }, [direction, totalPages, updateVisibleRange, visible]);

  // Map vertical wheel input to horizontal strip scroll. Without this, a
  // mouse-wheel gesture over the strip scrolls the underlying reader
  // (which is fixed-position behind the strip) and the strip itself stays
  // put — a jarring mismatch with the user's mental model.
  //
  // Why a manual native listener: React's synthetic `onWheel` is registered
  // passive by default since React 17, so `event.preventDefault()` from a
  // synthetic handler is a no-op. The non-passive flag has to be set at
  // `addEventListener` time. We also stop propagation to keep the gesture
  // from also nudging any ancestor scroll container that might exist.
  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      // Trackpad horizontal-swipes already produce `deltaX`; pass those
      // through unchanged. Only convert pure vertical wheel ticks.
      if (e.deltaY === 0) return;
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const indices = Array.from({ length: totalPages }, (_, i) => i);
  if (direction === "rtl") indices.reverse();

  return (
    <TooltipProvider delayDuration={350}>
      <nav
        aria-label="Page navigator"
        data-state={visible ? "open" : "closed"}
        aria-hidden={visible ? undefined : true}
        className="fixed inset-x-0 bottom-0 z-20 transition-transform duration-300 ease-out data-[state=closed]:pointer-events-none data-[state=closed]:translate-y-full motion-reduce:transition-none"
      >
        {/* Background bar — only as tall as a non-active thumb. The active
         * thumb scales upward (origin-bottom) and overflows above this bar
         * into transparent space, so it visually pops out of the strip. */}
        <div
          aria-hidden="true"
          className="pointer-events-none absolute inset-x-0 bottom-0 h-48 border-t border-neutral-800/80 bg-neutral-950/85 backdrop-blur"
        />
        <ol
          ref={stripRef}
          onScroll={updateVisibleRange}
          className="relative flex items-end gap-3 overflow-x-auto px-3 pt-28 pb-3"
        >
          {indices.map((i, p) => {
            const isActive = activePages.includes(i);
            // Anchor = first page of the active group. Used for the scroll
            // target so we always center on the leading half of a pair.
            const isAnchor = isActive && i === activePages[0];
            // Detect adjacency to a same-group active sibling in *visual*
            // (iteration) order — handles RTL implicitly since `indices`
            // is already reversed for RTL above.
            const prevIdx = indices[p - 1];
            const nextIdx = indices[p + 1];
            const prevActive =
              prevIdx !== undefined && activePages.includes(prevIdx);
            const nextActive =
              nextIdx !== undefined && activePages.includes(nextIdx);
            const visualIndex = direction === "rtl" ? totalPages - 1 - i : i;
            const inViewport =
              visualIndex >= visibleRange.start &&
              visualIndex <= visibleRange.end;
            const shouldLoad = visible && (isActive || inViewport);
            const isDouble = pages[i]?.double_page === true;
            const button = (
              <button
                ref={isAnchor ? activeRef : undefined}
                type="button"
                onClick={() => setPage(i)}
                aria-label={`Jump to page ${i + 1}${isDouble ? " (double-page)" : ""}`}
                aria-current={isActive ? "page" : undefined}
                className={`group relative block origin-bottom overflow-visible transition-transform duration-200 ease-out motion-reduce:transition-none ${
                  isActive ? "scale-[1.6]" : ""
                }`}
              >
                {/* Border/ring/shadow live on this inner wrapper so they
                 * frame the thumbnail only — the page number sits below
                 * untouched. `object-cover` keeps the image flush to the
                 * border on a 2:3 box (matches a typical comic page); the
                 * occasional non-2:3 page accepts a small center crop in
                 * exchange for the cleaner frame. */}
                <span
                  className={`block overflow-hidden rounded-lg border-2 bg-neutral-900 transition-[border-color] duration-200 ease-out motion-reduce:transition-none ${
                    isActive
                      ? "border-accent"
                      : "border-neutral-700/80 group-hover:border-neutral-500"
                  }`}
                >
                  {shouldLoad ? (
                    /* eslint-disable-next-line @next/next/no-img-element */
                    <img
                      src={`/api/issues/${issueId}/pages/${i}/thumb?variant=strip`}
                      alt=""
                      loading="lazy"
                      decoding="async"
                      width={isDouble ? 192 : 96}
                      className={`block h-36 ${isDouble ? "w-48" : "w-24"} object-cover`}
                    />
                  ) : (
                    <span
                      className={`block h-36 ${isDouble ? "w-48" : "w-24"}`}
                    />
                  )}
                  <MarkerDots kinds={markerKindsByPage.get(i)} />
                </span>
                <span className="block py-0.5 text-center text-[11px] text-neutral-400">
                  {i + 1}
                </span>
              </button>
            );
            // Active li is given extra horizontal margin so neighbors slide
            // apart to make room for the larger active thumb. The margin
            // transition runs at the same duration as the scale so the slide
            // and the pop look like a single gesture.
            //
            // Outer edge of any active thumb keeps `m-5` (20px) to match
            // the historic single-active spacing. The inner edge between
            // two paired halves needs more room — each scaled thumb
            // extends 28.8px past its box, so without a bump the two
            // would overlap. `m-7` (28px) per side + `gap-3` (12px) leaves
            // ~10px of visual gap that reads as the binding line of a
            // printed spread.
            const liMarginClass = !isActive
              ? ""
              : `relative z-10 ${prevActive ? "ml-7" : "ml-5"} ${
                  nextActive ? "mr-7" : "mr-5"
                }`;
            return (
              <li
                key={i}
                className={`shrink-0 transition-[margin] duration-200 ease-out motion-reduce:transition-none ${liMarginClass}`}
              >
                {shouldLoad ? (
                  <Tooltip>
                    <TooltipTrigger asChild>{button}</TooltipTrigger>
                    <TooltipContent side="top" sideOffset={8}>
                      Page {i + 1}
                      {isDouble ? " · double-page" : ""}
                    </TooltipContent>
                  </Tooltip>
                ) : (
                  button
                )}
              </li>
            );
          })}
        </ol>
      </nav>
    </TooltipProvider>
  );
}

/** Tiny color-coded dots in the bottom-right of a page thumbnail
 *  signaling that the user has at least one marker of each kind on
 *  that page. Renders nothing when the set is empty. */
function MarkerDots({ kinds }: { kinds: Set<DotKind> | undefined }) {
  if (!kinds || kinds.size === 0) return null;
  return (
    <span
      aria-label="Has markers"
      className="pointer-events-none absolute right-1 bottom-1 flex items-center gap-0.5"
    >
      {KIND_DISPLAY_ORDER.filter((k) => kinds.has(k)).map((k) => (
        <span
          key={k}
          className={`block h-1.5 w-1.5 rounded-full ring-1 ring-black/30 ${KIND_DOT_BG[k]}`}
        />
      ))}
    </span>
  );
}
