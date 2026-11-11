"use client";

import { RetryingImg } from "@/components/RetryingImg";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { useReaderStore } from "@/lib/reader/store";
import {
  computeSpreadGroups,
  groupIndexForPage,
  nextPageSpreadMode,
  pageSpreadMode,
  withPageSpreadMode,
  type PageSpreadMode,
} from "@/lib/reader/spreads";
import { useSpreadOverrides } from "@/lib/reader/use-spread-overrides";
import { useIssueMarkers } from "@/lib/api/queries";
import { useSetIssuePageOverrides } from "@/lib/api/mutations";
import type { Direction } from "@/lib/reader/detect";
import type { MarkerKind, PageInfo } from "@/lib/api/types";
import { SWIPE_IGNORE_ATTR } from "@/lib/reader/use-swipe";
import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import { withContentVersion } from "@/lib/urls";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/folio/tooltip";

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

/** Strip geometry. `slot` / `wideSlot` are a thumb's layout width plus
 *  the row gap (px), for the scroll virtualizer; they must match the
 *  classes. The active thumb's margins make room for its pop: each scaled
 *  thumb overhangs its box by w·(scale−1)/2, and a spread pair needs a
 *  bit more between its halves so they read as a binding line. */
const REGULAR = {
  w: 96,
  wideW: 192,
  slot: 108, // w-24 + gap-3
  wideSlot: 204,
  thumbW: "w-24",
  thumbWideW: "w-48",
  thumbH: "h-36",
  /** Thumb scale by distance from the open page: 0 (open) … then 1. */
  scales: [1.6, 1.32, 1.15, 1.06],
  gap: 12,
  row: "gap-3 pt-28",
  bar: "h-[calc(12rem+var(--safe-bottom))]",
} as const;
const COMPACT = {
  w: 64,
  wideW: 128,
  slot: 72, // w-16 + gap-2
  wideSlot: 136,
  thumbW: "w-16",
  thumbWideW: "w-32",
  thumbH: "h-24",
  scales: [1.3, 1.16, 1.07],
  gap: 8,
  row: "gap-2 pt-10",
  bar: "h-[calc(8.25rem+var(--safe-bottom))]",
} as const;

/** Opacity by distance from the open page; further pages sit back. */
const THUMB_OPACITY = [1, 0.9, 0.8] as const;
const THUMB_OPACITY_REST = 0.7;

/**
 * Pure: how a strip thumb is drawn at `distance` pages from the nearest
 * open page — the strip swells toward the open page like a magnifier.
 * `overhang` is how far the scaled thumb (base width `width`) reaches
 * past its box on each side; the slot's margins take it up so neighbours
 * keep an even gap instead of overlapping. Exported for tests.
 */
export function stripThumbLook(
  distance: number,
  scales: readonly number[],
  width: number,
): { scale: number; opacity: number; overhang: number } {
  const scale = scales[distance] ?? 1;
  return {
    scale,
    opacity: THUMB_OPACITY[distance] ?? THUMB_OPACITY_REST,
    overhang: (width * (scale - 1)) / 2,
  };
}

const COMPACT_QUERY = "(max-width: 639px), (max-height: 499px)";

/** Whether the strip uses its compact size (phones, short landscape). */
function useCompactStrip(): boolean {
  return useSyncExternalStore(
    (onChange) => {
      const mql = window.matchMedia?.(COMPACT_QUERY);
      mql?.addEventListener("change", onChange);
      return () => mql?.removeEventListener("change", onChange);
    },
    () => window.matchMedia?.(COMPACT_QUERY).matches ?? false,
    () => false,
  );
}

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
  urlVersion,
}: {
  issueId: string;
  totalPages: number;
  currentPage: number;
  direction: Direction;
  pages: PageInfo[];
  /** Archive-content `?v=` stamp for thumb URLs (see Reader). */
  urlVersion: string | null;
}) {
  const setPage = useReaderStore((s) => s.setPage);
  const visible = useReaderStore((s) => s.pageStripVisible);
  // Phones and short landscape windows get smaller thumbnails and a
  // smaller pop, so the strip covers far less of the page there.
  const compact = useCompactStrip();
  const size = compact ? COMPACT : REGULAR;
  const viewMode = useReaderStore((s) => s.viewMode);
  const coverSolo = useReaderStore((s) => s.coverSolo);
  // WP-4.3 manual spread controls — same query cache the reader pairs
  // from, so a toggle here re-pairs the open spread on the same frame.
  const spreadOverrides = useSpreadOverrides(issueId);
  const setOverrides = useSetIssuePageOverrides(issueId);
  const cycleSpreadMode = useCallback(
    (page: number) => {
      const next = nextPageSpreadMode(pageSpreadMode(spreadOverrides, page));
      setOverrides.mutate(withPageSpreadMode(spreadOverrides, page, next));
    },
    [spreadOverrides, setOverrides],
  );
  // Wide (two-slot) thumbnail: the `double_page` flag, unless the user
  // forced the page single; or a manual forced spread.
  const isWideThumb = useCallback(
    (i: number) => {
      const mode = pageSpreadMode(spreadOverrides, i);
      if (mode === "spread") return true;
      if (mode === "single") return false;
      return pages[i]?.double_page === true;
    },
    [spreadOverrides, pages],
  );

  // Keep the thumbnail images mounted through the slide-out animation.
  // When the user hides the chrome, `visible` flips to false immediately
  // but the `<nav>` takes 300ms to translate off-screen (the
  // `duration-300` transform below). Tearing the `<img>`s down on the
  // same tick would swap them for the gray placeholder boxes mid-slide —
  // most glaring on the scaled-up active thumb. Instead we hold the
  // images mounted until the strip's transform transition actually
  // finishes (`parked` = "settled off-screen"), so they ride the strip
  // down and only unmount once it's out of view. Under reduced motion
  // there's no transition to wait on, so the close unmounts immediately
  // (correct — nothing slides).
  const [parked, setParked] = useState(!visible);
  const renderThumbs = visible || !parked;

  // v0.3.44 entrance polish: gate the open `data-state` on a first-
  // frame mount flip so the strip slides up from the viewport
  // bottom on initial reader open (when the user has the strip
  // preference on) instead of materializing at full height. After
  // first paint this becomes a no-op and `visible` drives the
  // toggle as before.
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setMounted(true));
    return () => cancelAnimationFrame(raf);
  }, []);

  // Pre-warm strip thumbnails around the current page so the page map is
  // already cached — and server-side generation already kicked off —
  // before the strip slides up. Without this, the visible thumbs only
  // start loading once the strip opens, showing the placeholder box
  // briefly before filling. Bounded window around the current page;
  // pages further out warm lazily as the user scrolls the strip.
  const warmedThumbs = useRef<Set<number>>(new Set());
  useEffect(() => {
    const WINDOW = 12;
    const lo = Math.max(0, currentPage - WINDOW);
    const hi = Math.min(totalPages - 1, currentPage + WINDOW);
    for (let i = lo; i <= hi; i += 1) {
      if (warmedThumbs.current.has(i)) continue;
      warmedThumbs.current.add(i);
      const img = new Image();
      img.fetchPriority = "low";
      img.src = withContentVersion(
        `/issues/${issueId}/pages/${i}/thumb?variant=strip`,
        urlVersion,
      );
    }
  }, [issueId, currentPage, totalPages, urlVersion]);
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
  // Touch has no hover, so the hover-revealed spread pills would be
  // unreachable there: on a coarse primary pointer every pill is shown
  // (audit N-R11). Read once — the strip is a `ssr: false` chunk, so
  // there is no server snapshot to disagree with.
  const coarsePointer = useMemo(() => primaryPointerIsCoarse(), []);
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
      const w = isWideThumb(i) ? size.wideSlot : size.slot;
      offsets[i + 1] = offsets[i]! + w;
    }
    return offsets;
  }, [isWideThumb, totalPages, size]);

  // Pages currently visible in the reader. In single/webtoon this is just
  // the current page; in double-page view it's the full spread group, so a
  // pair `[i, i+1]` lights up both thumbnails. Cover-solo and aspect-ratio
  // spreads (`double_page === true`) collapse back to a single active thumb
  // because `computeSpreadGroups` already emits them as solo groups.
  const activePages = useMemo<readonly number[]>(() => {
    if (viewMode !== "double") return [currentPage];
    const groups = computeSpreadGroups(pages, {
      coverSolo,
      totalPages,
      overrides: spreadOverrides,
    });
    const idx = groupIndexForPage(groups, currentPage);
    return groups[idx] ?? [currentPage];
  }, [viewMode, coverSolo, pages, currentPage, totalPages, spreadOverrides]);

  // Every slot's look and margins, plus where its centre lands along the
  // row (excluding the row's start padding) once the margin transitions
  // settle. Centring scrolls to that final spot directly; measuring the
  // DOM mid-transition would aim at a stale layout.
  const geometry = useMemo(() => {
    const order = Array.from({ length: totalPages }, (_, i) => i);
    if (direction === "rtl") order.reverse();
    const slots = new Array<{
      look: ReturnType<typeof stripThumbLook>;
      marginLeft: number;
      marginRight: number;
      center: number;
    }>(totalPages);
    // Each slot's margins take up its thumb's overhang, so the row keeps
    // an even gap however much each thumb is scaled; the margins
    // transition with the scale, so neighbours glide apart as the
    // magnified region moves. Between the two halves of an open spread
    // the gap narrows a little, reading as the binding.
    const binding = size.gap / 3;
    let x = 0;
    order.forEach((i, p) => {
      const isActive = activePages.includes(i);
      const prev = order[p - 1];
      const next = order[p + 1];
      const prevActive =
        isActive && prev !== undefined && activePages.includes(prev);
      const nextActive =
        isActive && next !== undefined && activePages.includes(next);
      // Rendered width: the thumb plus its 2px border each side.
      const width = (isWideThumb(i) ? size.wideW : size.w) + 4;
      const distance = Math.min(...activePages.map((a) => Math.abs(a - i)));
      const look = stripThumbLook(distance, size.scales, width);
      const marginLeft = look.overhang - (prevActive ? binding : 0);
      const marginRight = look.overhang - (nextActive ? binding : 0);
      if (p > 0) x += size.gap;
      x += marginLeft;
      const center = x + width / 2;
      x += width + marginRight;
      slots[i] = { look, marginLeft, marginRight, center };
    });
    return { order, slots };
  }, [totalPages, direction, activePages, isWideThumb, size]);

  /** Scroll so the open page sits in the middle of the strip. */
  const centerOpenPage = useCallback(
    (behavior: ScrollBehavior) => {
      const el = stripRef.current;
      const anchor = activePages[0];
      const slot = anchor === undefined ? undefined : geometry.slots[anchor];
      if (!el || !slot) return;
      const padLeft = parseFloat(getComputedStyle(el).paddingLeft) || 0;
      const left = padLeft + slot.center - el.clientWidth / 2;
      if (typeof el.scrollTo === "function") el.scrollTo({ left, behavior });
      else el.scrollLeft = left;
    },
    [activePages, geometry],
  );

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
    const reduced =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    const isFirstScroll = !hasCenteredForOpenRef.current;
    centerOpenPage(reduced || isFirstScroll ? "auto" : "smooth");
    hasCenteredForOpenRef.current = true;
    requestAnimationFrame(updateVisibleRange);
  }, [currentPage, centerOpenPage, updateVisibleRange, visible]);

  useEffect(() => {
    if (!visible) return;
    updateVisibleRange();
  }, [direction, totalPages, updateVisibleRange, visible]);

  // A rotation or window resize reflows the row (and may switch between
  // the regular and compact sizes): keep the open page centred.
  useEffect(() => {
    if (!visible) return;
    const onResize = () => {
      centerOpenPage("auto");
      requestAnimationFrame(updateVisibleRange);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [visible, centerOpenPage, updateVisibleRange]);

  // Switching between the regular and compact sizes changes the whole
  // geometry: re-centre at once rather than smooth-panning.
  useEffect(() => {
    if (visible) centerOpenPage("auto");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [compact]);

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

  const indices = geometry.order;

  return (
    <TooltipProvider delayDuration={350}>
      <nav
        aria-label="Page navigator"
        // Opt the whole strip out of the reader's page-turn/pan drag
        // (use-swipe.ts): the thumbnail rail scrolls horizontally
        // itself, and under the touch-event binding a strip scroll
        // would otherwise ALSO run the container drag — turning the
        // page on lift and snapping the freshly-scrolled strip back
        // to the new page's thumb.
        {...{ [SWIPE_IGNORE_ATTR]: "" }}
        data-state={mounted && visible ? "open" : "closed"}
        aria-hidden={mounted && visible ? undefined : true}
        // `inert` while closed: the strip holds up to `totalPages`
        // thumbnail buttons — without it, a 200-page issue put ~200
        // invisible tab stops behind the translate (WCAG 4.1.2).
        inert={mounted && visible ? undefined : true}
        // Drop the lingering thumbnails once the strip has finished
        // sliding off-screen. Guard to the nav's own slide transition —
        // the thumbnails/li margins inside also transition and bubble
        // their `transitionend` up here, so filter on `currentTarget`.
        //
        // The animated property is `translate`, NOT `transform`: in
        // Tailwind v4 the `translate-y-full` utility emits the dedicated
        // `translate:` CSS property (and `transition-transform` resolves
        // to `transition-property: transform, translate, scale, rotate`).
        // So the `transitionend` fires with `propertyName === "translate"`.
        // The original guard checked `=== "transform"` (v3 semantics,
        // where translate composed into `transform`), which never matched
        // under v4 — `parked` stayed stuck at its initial value, and since
        // the strip starts hidden (`pageStripVisible` defaults to false)
        // that initial value is `true`, so the first close tore the
        // thumbnails down on the same tick and re-exposed the gray boxes.
        // Accept both names so a future utility/property swap can't
        // silently re-break it.
        onTransitionEnd={(e) => {
          if (
            e.target === e.currentTarget &&
            (e.propertyName === "translate" || e.propertyName === "transform")
          ) {
            setParked(!visible);
          }
        }}
        className="fixed inset-x-0 bottom-0 z-20 transition-transform duration-300 ease-out data-[state=closed]:pointer-events-none data-[state=closed]:translate-y-full motion-reduce:transition-none"
      >
        {/* Background bar — only as tall as a non-active thumb. The active
         * thumb scales upward (origin-bottom) and overflows above this bar
         * into transparent space, so it visually pops out of the strip.
         * Height grows by `--safe-bottom` so the bar still visually
         * fills the home-indicator area on iPhones in standalone
         * mode (where `viewport-fit: cover` lets us paint behind it). */}
        <div
          aria-hidden="true"
          className={`pointer-events-none absolute inset-x-0 bottom-0 border-t border-neutral-800/80 bg-neutral-950/85 backdrop-blur ${size.bar}`}
        />
        <ol
          ref={stripRef}
          onScroll={updateVisibleRange}
          // `pb-3` floor + `--safe-bottom` so thumbnails sit above the
          // home indicator. Horizontal insets clear landscape notches.
          // Edges fade out so it reads as a row that scrolls on.
          className={`page-strip-fade relative flex items-end overflow-x-auto pr-[max(0.75rem,var(--safe-right))] pb-[calc(0.75rem+var(--safe-bottom))] pl-[max(0.75rem,var(--safe-left))] ${size.row}`}
        >
          {indices.map((i) => {
            const isActive = activePages.includes(i);
            // Anchor = first page of the active group. Used for the scroll
            // target so we always center on the leading half of a pair.
            const isAnchor = isActive && i === activePages[0];
            const visualIndex = direction === "rtl" ? totalPages - 1 - i : i;
            const inViewport =
              visualIndex >= visibleRange.start &&
              visualIndex <= visibleRange.end;
            const shouldLoad = renderThumbs && (isActive || inViewport);
            const isDouble = isWideThumb(i);
            const spreadMode = pageSpreadMode(spreadOverrides, i);
            const { look, marginLeft, marginRight } = geometry.slots[i]!;
            const button = (
              <button
                ref={isAnchor ? activeRef : undefined}
                type="button"
                onClick={() => setPage(i)}
                aria-label={`Jump to page ${i + 1}${isDouble ? " (double-page)" : ""}`}
                aria-current={isActive ? "page" : undefined}
                style={{ scale: String(look.scale) }}
                className="group focus-visible:ring-accent relative block origin-bottom overflow-visible rounded-lg transition-[scale] duration-300 ease-out focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:ring-offset-neutral-950 focus-visible:outline-none motion-reduce:transition-none"
              >
                {/* Border/ring/shadow live on this inner wrapper so they
                 * frame the thumbnail only — the page number sits below
                 * untouched. `object-cover` keeps the image flush to the
                 * border on a 2:3 box (matches a typical comic page); the
                 * occasional non-2:3 page accepts a small center crop in
                 * exchange for the cleaner frame. */}
                <span
                  // Pages other than the open one sit back a little, so the
                  // current page is what the eye finds first.
                  className={`block overflow-hidden rounded-lg border-2 bg-neutral-900 transition-[border-color,opacity] duration-200 ease-out motion-reduce:transition-none ${
                    isActive
                      ? "border-accent"
                      : "border-neutral-700/80 opacity-(--thumb-o) group-hover:border-neutral-500 group-hover:opacity-100 group-focus-visible:opacity-100"
                  }`}
                >
                  {shouldLoad ? (
                    <RetryingImg
                      src={withContentVersion(
                        `/issues/${issueId}/pages/${i}/thumb?variant=strip`,
                        urlVersion,
                      )}
                      alt=""
                      // Eager: `shouldLoad` already gates rendering to the
                      // visible + overscan window, so we want these to fetch
                      // immediately on open rather than wait for the browser's
                      // lazy heuristic (which adds the blank-then-fill beat).
                      loading="eager"
                      decoding="async"
                      width={isDouble ? size.wideW : size.w}
                      className={`block ${size.thumbH} ${isDouble ? size.thumbWideW : size.thumbW} object-cover`}
                    />
                  ) : (
                    <span
                      className={`block ${size.thumbH} ${isDouble ? size.thumbWideW : size.thumbW}`}
                    />
                  )}
                  <MarkerDots kinds={markerKindsByPage.get(i)} />
                </span>
                <span className="block py-0.5 text-center text-xs text-neutral-400">
                  {i + 1}
                </span>
              </button>
            );
            return (
              <ThumbSlot
                key={i}
                style={
                  {
                    marginLeft,
                    marginRight,
                    "--thumb-o": look.opacity,
                  } as React.CSSProperties
                }
                className={`group/thumb relative shrink-0 transition-[margin] duration-300 ease-out motion-reduce:transition-none ${
                  isActive ? "z-10" : ""
                }`}
              >
                {(focused) => (
                  <>
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
                    {/* After the thumb in DOM order (positioned over it),
                     * so Tab from a focused thumb lands on that page's
                     * pill. Rendered for every thumb, not just the loaded
                     * window: it is one small button, and an overridden
                     * page's pill must be a tab stop wherever the strip
                     * is scrolled. */}
                    {viewMode === "double" ? (
                      <SpreadModeToggle
                        page={i}
                        mode={spreadMode}
                        active={isActive}
                        focused={focused}
                        coarsePointer={coarsePointer}
                        onCycle={cycleSpreadMode}
                      />
                    ) : null}
                  </>
                )}
              </ThumbSlot>
            );
          })}
        </ol>
      </nav>
    </TooltipProvider>
  );
}

/**
 * One strip slot. Owns the "does this thumb (or its pill) hold focus"
 * flag so a keyboard user Tabbing along the strip re-renders one slot at
 * a time, not every thumbnail; the render-prop child receives the flag.
 */
function ThumbSlot({
  className,
  style,
  children,
}: {
  className: string;
  style?: React.CSSProperties;
  children: (focused: boolean) => ReactNode;
}) {
  const [focused, setFocused] = useState(false);
  return (
    <li
      onFocus={() => setFocused(true)}
      onBlur={(e) => {
        // Focus moving between the thumb and its own pill stays inside
        // the slot; only a departure clears the flag.
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setFocused(false);
      }}
      className={className}
      style={style}
    >
      {children(focused)}
    </li>
  );
}

const SPREAD_MODE_LABEL: Record<PageSpreadMode, string> = {
  auto: "Auto",
  spread: "Spread",
  single: "Single",
};

const SPREAD_MODE_DESCRIPTION: Record<PageSpreadMode, string> = {
  auto: "automatic pairing",
  spread: "forced spread (shown alone)",
  single: "forced single page (pairs with a neighbour)",
};

/**
 * WP-4.3 strip affordance: a small pill over a thumbnail's bottom-left
 * corner that cycles the page's manual spread mode
 * (auto → spread → single → auto). Double-page view only.
 *
 * Shown on overridden pages, on the pages currently on screen, and on
 * every page when the primary pointer is coarse (touch has no hover to
 * reveal it with); otherwise revealed on hover or focus. Tab stops are
 * the overridden and on-screen pages' pills plus the pill of whichever
 * thumb holds focus — so every page's pairing is one Tab away from its
 * thumb without the strip doubling its tab order (audit N-R11). The
 * `aria-label` carries the current and the next state.
 *
 * `folio-tap` gives touch the standard 44px tap area (phone-layout rule
 * 9) without growing the pill. That area reaches ~13px past the pill
 * into the thumb's lower-left corner — a visible control claiming the
 * space next to it, on a 96×164px thumb that stays easy to hit
 * elsewhere; the alternative, a 20px-tall target, fails the rule.
 */
function SpreadModeToggle({
  page,
  mode,
  active,
  focused,
  coarsePointer,
  onCycle,
}: {
  page: number;
  mode: PageSpreadMode;
  active: boolean;
  /** The page's thumb or pill holds keyboard focus. */
  focused: boolean;
  /** Primary pointer is coarse (touch): always show the pill. */
  coarsePointer: boolean;
  onCycle: (page: number) => void;
}) {
  const next = nextPageSpreadMode(mode);
  const overridden = mode !== "auto";
  const shown = overridden || active || coarsePointer;
  const tabbable = overridden || active || focused;
  return (
    <button
      type="button"
      tabIndex={tabbable ? 0 : -1}
      onClick={(e) => {
        e.stopPropagation();
        onCycle(page);
      }}
      aria-label={`Page ${page + 1} pairing: ${SPREAD_MODE_DESCRIPTION[mode]}. Change to ${SPREAD_MODE_DESCRIPTION[next]}.`}
      title={`Pairing: ${SPREAD_MODE_DESCRIPTION[mode]} — click for ${SPREAD_MODE_LABEL[next].toLowerCase()}`}
      data-spread-mode={mode}
      className={`folio-tap focus-visible:ring-accent absolute bottom-7 left-1 z-20 rounded-md px-1.5 py-0.5 text-xs leading-none font-medium ring-1 transition-opacity duration-150 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:outline-none motion-reduce:transition-none ${
        mode === "auto"
          ? "bg-neutral-950/80 text-neutral-300 ring-neutral-700"
          : "bg-accent text-accent-foreground ring-accent"
      } ${
        shown
          ? "opacity-100"
          : "opacity-0 group-focus-within/thumb:opacity-100 group-hover/thumb:opacity-100"
      }`}
    >
      {SPREAD_MODE_LABEL[mode]}
    </button>
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
