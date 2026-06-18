"use client";

import * as React from "react";
import Link from "next/link";

import { CblStatsPills } from "@/components/cbl/CblStatsPills";
import { EmptyState } from "@/components/ui/empty-state";
import { CblWindowCard } from "@/components/cbl/CblWindowCard";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { CoverPriorityProvider } from "@/components/library/cover-priority";
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { RailIconPicker } from "@/components/library/RailIconPicker";
import {
  useCblListWindowInfinite,
  useCollectionEntries,
  useSavedViewResults,
} from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

import {
  ContinueReadingRailBody,
  OnDeckRailBody,
  useSystemRailIsEmpty,
} from "./system-rails";

/** URL for the rail's "View all" affordance + header link.
 *
 *  - System rails get a kebab-case alias (`/views/continue-reading`,
 *    `/views/on-deck`) so the URL is readable when the user clicks the
 *    rail header from the home page. The `[id]` route maps these back
 *    to the row's `system_key` server-side.
 *  - User-authored views use their UUID id. */
function viewDetailHref(view: SavedViewView): string {
  if (view.kind === "system" && view.system_key) {
    return `/views/${view.system_key.replace(/_/g, "-")}`;
  }
  return `/views/${view.id}`;
}

/** Max cards we hydrate on the home page for filter rails. Higher = more
 *  cards visible before the trailing "View all" tile becomes the way to
 *  reach the long tail. */
const RAIL_PREVIEW_LIMIT = 30;

/** Read-only home-page rail. Pinning, reordering, edit, and delete all
 *  live on the management page (`/settings/views`); the home page is
 *  pure presentation.
 *
 *  `cardSize` is the home-page density slider value in pixels. Each
 *  card wrapper sets its width inline so the same toggle that drives
 *  the library grid also drives rail card widths. */
export function SavedViewRail({
  view,
  cardSize,
  priority = false,
}: {
  view: SavedViewView;
  cardSize: number;
  /** First/above-the-fold rail: eager-load + high-priority its covers so
   *  the LCP image isn't deferred. Off for every other rail. */
  priority?: boolean;
}) {
  // System rails (Continue reading / On deck) hide themselves entirely when
  // there's nothing to show — no point rendering an empty header on a
  // fresh account that hasn't started reading anything yet.
  const systemKey = view.kind === "system" ? (view.system_key ?? "") : "";
  const systemEmpty = useSystemRailIsEmpty(systemKey);
  if (view.kind === "system" && systemEmpty) return null;

  const itemStyle: React.CSSProperties = { width: `${cardSize}px` };
  // "View all" trailing tile fires for surfaces that have a finite preview
  // but a deeper detail page. Suppressed when the rail body says it's
  // already showing everything (returns hasMore: false).
  const railContent = renderRailBody(view, itemStyle);
  return (
    <section
      className="flex flex-col"
      // Header → cards gap follows the density token so compact mode
      // tightens not just rail-to-rail spacing but also the rail's
      // own internal rhythm.
      style={{ gap: "var(--density-rail-inner-gap)" }}
    >
      <RailHeader view={view} />
      <CoverPriorityProvider value={priority}>
        <HorizontalScrollRail
          viewAllHref={railContent.hasMore ? viewDetailHref(view) : undefined}
          itemWidthPx={cardSize}
        >
          {railContent.body}
        </HorizontalScrollRail>
      </CoverPriorityProvider>
    </section>
  );
}

/** Dispatch the rail-body branch for a given view + report whether the
 *  rail is showing a strict subset of its data (so the parent renders
 *  the trailing "View all" tile only when there's more to see). */
function renderRailBody(
  view: SavedViewView,
  itemStyle: React.CSSProperties,
): { body: React.ReactNode; hasMore: boolean } {
  if (view.kind === "system" && view.system_key === "continue_reading") {
    return {
      body: <ContinueReadingRailBody itemStyle={itemStyle} />,
      hasMore: true,
    };
  }
  if (view.kind === "system" && view.system_key === "on_deck") {
    return {
      body: <OnDeckRailBody itemStyle={itemStyle} />,
      hasMore: true,
    };
  }
  if (view.kind === "filter_series") {
    return {
      body: <FilterRailBody view={view} itemStyle={itemStyle} />,
      hasMore: true,
    };
  }
  if (view.kind === "collection") {
    return {
      body: <CollectionRailBody view={view} itemStyle={itemStyle} />,
      hasMore: true,
    };
  }
  if (view.cbl_list_id) {
    return {
      body: (
        <CblRailBody
          view={view}
          listId={view.cbl_list_id}
          itemStyle={itemStyle}
        />
      ),
      hasMore: true,
    };
  }
  return { body: null, hasMore: false };
}

function RailHeader({ view }: { view: SavedViewView }) {
  const isCbl = view.kind === "cbl";
  const href = viewDetailHref(view);
  // The trailing "View all" tile at the end of the rail (added by
  // `<HorizontalScrollRail>`) now serves that affordance, so the
  // duplicate right-aligned link that used to live here has been
  // removed. The title itself stays clickable for users who reach for
  // the rail name.
  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <RailIconPicker view={view} />
      {/* Each rail is a home-page section — an `<h2>` gives the page a real
          outline (h1 → rail h2s) instead of h1-then-flat (audit E9). The
          title stays a link inside the heading. */}
      <h2 className="min-w-0 truncate text-lg font-semibold tracking-tight">
        <Link
          href={href}
          className="hover:text-foreground block truncate"
          title={view.name}
        >
          {view.name}
        </Link>
      </h2>
      {isCbl && view.cbl_list_id ? (
        <CblStatsPills cblListId={view.cbl_list_id} size="rail" />
      ) : null}
    </div>
  );
}

function FilterRailBody({
  view,
  itemStyle,
}: {
  view: SavedViewView;
  itemStyle: React.CSSProperties;
}) {
  const results = useSavedViewResults(view.id);
  if (results.isLoading) {
    return (
      <>
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} style={itemStyle} className="shrink-0">
            <SeriesCardSkeleton size="md" />
          </div>
        ))}
      </>
    );
  }
  const items = results.data?.items ?? [];
  if (items.length === 0)
    return <RailEmptyState message="Nothing matches yet." />;
  return (
    <>
      {items.slice(0, RAIL_PREVIEW_LIMIT).map((s) => (
        <div key={s.id} style={itemStyle} className="shrink-0">
          <SeriesCard series={s} size="md" />
        </div>
      ))}
    </>
  );
}

function CblRailBody({
  view,
  listId,
  itemStyle,
}: {
  view: SavedViewView;
  listId: string;
  itemStyle: React.CSSProperties;
}) {
  // Reading-window query: 3 finished entries on the left for context,
  // a wider upcoming tail on the right so users can browse several
  // issues ahead without bouncing to the detail page. The infinite
  // variant lets the rail asynchronously extend in either direction
  // as the user scrolls past the edge sentinels, without disturbing
  // the anchored slice that landed first.
  const query = useCblListWindowInfinite(listId, {
    before: 3,
    after: 24,
    limit: 24,
  });

  // Latest query stashed in a ref so the IntersectionObserver callbacks
  // can read `hasNextPage` / `isFetching*` without us having to rebuild
  // the observer (and re-fire its initial intersection check) every
  // time TanStack swaps in a new query object. The ref is updated in
  // an effect, not at render time — writing during render would trip
  // react-hooks/refs (and risks stale closures during concurrent
  // renders); the callbacks are always async so the brief
  // render-to-effect gap is invisible.
  const queryRef = React.useRef(query);
  React.useEffect(() => {
    queryRef.current = query;
  });

  // Each rendered sentinel is keyed inside the items map by its
  // neighbour's `issue.id`, so React unmounts and remounts the DOM
  // node whenever pages prepend or append. Callback refs let us
  // re-attach the IntersectionObserver to the fresh node without
  // tearing down the entire effect on every fetch.
  const railScrollRef = React.useRef<HTMLElement | null>(null);
  const leftObsRef = React.useRef<IntersectionObserver | null>(null);
  const rightObsRef = React.useRef<IntersectionObserver | null>(null);

  const setLeftSentinel = React.useCallback((node: HTMLDivElement | null) => {
    leftObsRef.current?.disconnect();
    leftObsRef.current = null;
    if (!node) return;
    // Cache the closest horizontally-scrolling ancestor once — that's
    // the `<div>` inside HorizontalScrollRail whose `scrollLeft` we'll
    // bump on every prepend to keep the user's visible cards anchored.
    if (!railScrollRef.current) {
      let cur: HTMLElement | null = node.parentElement;
      while (cur) {
        const overflow = window.getComputedStyle(cur).overflowX;
        if (overflow === "auto" || overflow === "scroll") {
          railScrollRef.current = cur;
          break;
        }
        cur = cur.parentElement;
      }
    }
    const obs = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        const q = queryRef.current;
        if (q.hasPreviousPage && !q.isFetchingPreviousPage) {
          void q.fetchPreviousPage();
        }
      },
      { threshold: 0 },
    );
    obs.observe(node);
    leftObsRef.current = obs;
  }, []);

  const setRightSentinel = React.useCallback((node: HTMLDivElement | null) => {
    rightObsRef.current?.disconnect();
    rightObsRef.current = null;
    if (!node) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        const q = queryRef.current;
        if (q.hasNextPage && !q.isFetchingNextPage) {
          void q.fetchNextPage();
        }
      },
      { threshold: 0 },
    );
    obs.observe(node);
    rightObsRef.current = obs;
  }, []);

  // Final disconnect on unmount — callback refs handle re-attachment
  // mid-life, but the last node is unmounted without firing the
  // ref(null) callback in StrictMode dev cleanups.
  React.useEffect(() => {
    return () => {
      leftObsRef.current?.disconnect();
      rightObsRef.current?.disconnect();
    };
  }, []);

  // Derive the rendered item list before any early returns so the
  // layout-effect below has its dependency on every render — hooks
  // must run in a stable order across renders.
  const pages = query.data?.pages ?? [];
  // Flatten + de-dupe by position. ACL filtering can make two adjacent
  // pages share an entry at the boundary in rare cases; the Set guard
  // keeps the rendered set unique without re-sorting.
  const seen = new Set<number>();
  const items = pages.flatMap((p) =>
    p.items.filter((e) => {
      if (seen.has(e.position)) return false;
      seen.add(e.position);
      return true;
    }),
  );

  // Compensate scrollLeft on prepends. When fetchPreviousPage lands
  // and 24 new entries land before items[0], the rail's `scrollLeft`
  // is numerically unchanged but visually now points at the newly
  // prepended content — the user's visible cards appear to leap
  // rightward. Bumping `scrollLeft` by the total width of the
  // prepended content (card width + the rail's `gap-3` 12px gutter)
  // keeps the previously visible cards anchored to the same screen
  // position. Append (fetchNextPage) doesn't need compensation:
  // adding content past the right edge doesn't shift anything
  // currently visible. Lives in a `useLayoutEffect` so the fix
  // commits before the next paint — no perceptible jump.
  const lowestPosRef = React.useRef<number | null>(null);
  const cardSize = parseFloat(String(itemStyle.width ?? "0"));
  React.useLayoutEffect(() => {
    if (items.length === 0) return;
    const newLowest = items[0]!.position;
    const prev = lowestPosRef.current;
    lowestPosRef.current = newLowest;
    if (prev == null || newLowest >= prev) return;
    let prepended = 0;
    for (const it of items) {
      if (it.position >= prev) break;
      prepended++;
    }
    if (prepended === 0) return;
    const scroller = railScrollRef.current;
    if (!scroller || !Number.isFinite(cardSize) || cardSize <= 0) return;
    // gap-3 = 12px between flex children inside HorizontalScrollRail.
    scroller.scrollLeft += prepended * (cardSize + 12);
  }, [items, cardSize]);

  if (query.isLoading) {
    return (
      <>
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} style={itemStyle} className="shrink-0">
            <IssueCardSkeleton />
          </div>
        ))}
      </>
    );
  }
  if (items.length === 0) {
    const initialPage = pages[0];
    return (
      <RailEmptyState
        message={
          (initialPage?.total_matched ?? 0) === 0
            ? "No matched entries in this list."
            : "Nothing to show in this window."
        }
      />
    );
  }
  // Anchor info lives on the initial (first-loaded) page. As the user
  // walks backward via fetchPreviousPage, the initial page slides into
  // the middle of the array — anchorPos lets us recompute its slot.
  const initialPage = pages.find((p) => p.current_index != null);
  const anchorPos =
    initialPage && initialPage.current_index != null
      ? initialPage.items[initialPage.current_index]?.position
      : undefined;
  // Sentinels sit ~2 cards in from each end so the fetch starts while
  // the user still has runway between them and the data edge.
  // IntersectionObserver respects the rail's `overflow-x` clipping —
  // a sentinel placed at the absolute end wouldn't fire until the
  // edge is fully in view, defeating the "anticipatory load" intent.
  const sentinelOffset = Math.min(2, Math.max(0, items.length - 1));
  const leftSentinelIdx = sentinelOffset;
  const rightSentinelIdx = items.length - 1 - sentinelOffset;

  return (
    <>
      {items.map((entry, i) => {
        const isCurrent = anchorPos != null && entry.position === anchorPos;
        return (
          <React.Fragment key={entry.issue.id}>
            {i === leftSentinelIdx && (
              <div
                ref={setLeftSentinel}
                aria-hidden
                className="pointer-events-none w-0 shrink-0"
              />
            )}
            <div
              style={itemStyle}
              className="shrink-0"
              // The rail wrapper looks for this attribute on mount to
              // auto-scroll the current entry into view.
              data-rail-current={isCurrent ? "true" : undefined}
            >
              <CblWindowCard
                entry={entry}
                isCurrent={isCurrent}
                cblSavedViewId={view.id}
              />
            </div>
            {i === rightSentinelIdx && i !== leftSentinelIdx && (
              <div
                ref={setRightSentinel}
                aria-hidden
                className="pointer-events-none w-0 shrink-0"
              />
            )}
          </React.Fragment>
        );
      })}
    </>
  );
}

/** Collection rail body — paints the user's mixed series + issue
 *  entries via the same `<SeriesCard>` / `<IssueCard>` components the
 *  rest of the home page uses, so the kebab menu and play overlay
 *  surface identically on cards inside a pinned collection. Want to
 *  Read is just another collection at this layer. */
function CollectionRailBody({
  view,
  itemStyle,
}: {
  view: SavedViewView;
  itemStyle: React.CSSProperties;
}) {
  const entries = useCollectionEntries(view.id, { limit: RAIL_PREVIEW_LIMIT });
  if (entries.isLoading) {
    return (
      <>
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} style={itemStyle} className="shrink-0">
            <SeriesCardSkeleton size="md" />
          </div>
        ))}
      </>
    );
  }
  const items = entries.data?.items ?? [];
  if (items.length === 0) {
    return (
      <RailEmptyState
        message={
          view.system_key === "want_to_read"
            ? "Nothing on your Want to Read list yet."
            : "This collection is empty."
        }
      />
    );
  }
  return (
    <>
      {items.map((entry) => (
        <div key={entry.id} style={itemStyle} className="shrink-0">
          {entry.series ? (
            <SeriesCard series={entry.series} size="md" />
          ) : entry.issue ? (
            <IssueCard issue={entry.issue} />
          ) : null}
        </div>
      ))}
    </>
  );
}

function RailEmptyState({ message }: { message: string }) {
  return <EmptyState size="rail" description={message} />;
}
