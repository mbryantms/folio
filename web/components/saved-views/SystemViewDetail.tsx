"use client";

import * as React from "react";
import Link from "next/link";
import { ChevronRight, LayoutGrid, Sparkles } from "lucide-react";

import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import {
  OnDeckCard,
  OnDeckCardSkeleton,
} from "@/components/library/OnDeckCard";
import {
  ProgressIssueCard,
  ProgressIssueCardSkeleton,
} from "@/components/library/ProgressIssueCard";
import { useCardSize } from "@/components/library/use-card-size";
import {
  CardSizeFields,
  CardSizeOptions,
} from "@/components/library/CardSizeOptions";
import { PhoneControlsBar } from "@/components/library/PhoneControlsBar";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { SimilarSeriesCard } from "@/components/library/SimilarSeriesCard";
import {
  useContinueReading,
  useIssuesCrossListInfinite,
  useOnDeck,
  useOnThisDay,
  useRandomPicks,
  useRecentHistory,
  useSimilarRailInfinite,
} from "@/lib/api/queries";
import type {
  ContinueReadingCard,
  IssueSummaryView,
  OnDeckCard as OnDeckCardData,
  SavedViewView,
  SeriesView,
} from "@/lib/api/types";
import { seriesUrl } from "@/lib/urls";

/** Query filters for the New Issues detail grid: the FULL ingest-ordered
 *  issue list. Deliberately uncapped — the home rail's per-series cap is
 *  a preview digest; this page is the complete record (list-pagination
 *  completeness: a detail surface must never silently truncate). Module
 *  constant so both hook call sites hash to the same query key. */
const NEW_ISSUES_FILTERS = { sort: "created_at", order: "desc" } as const;

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 180;
const CARD_SIZE_STORAGE_KEY = "folio.systemView.cardSize";

/**
 * Full-page detail view for system rails (Continue reading / On deck).
 * Renders the same cards as the home rail but in a wrapping responsive
 * grid (so the entire dataset fits on one page) and exposes a density
 * slider that matches the home page's affordance.
 */
export function SystemViewDetail({ view }: { view: SavedViewView }) {
  const isContinueReading = view.system_key === "continue_reading";
  const isOnDeck = view.system_key === "on_deck";
  const isNewIssues = view.system_key === "new_issues";
  const isSimilar = view.system_key === "similar_series";
  const isRecentHistory = view.system_key === "recent_history";
  const isRandomPicks = view.system_key === "random_picks";
  const isOnThisDay = view.system_key === "on_this_day";

  const cr = useContinueReading({ enabled: isContinueReading });
  const od = useOnDeck({ enabled: isOnDeck });
  const rh = useRecentHistory({ enabled: isRecentHistory });
  const rp = useRandomPicks({ enabled: isRandomPicks });
  const otd = useOnThisDay({ enabled: isOnThisDay });
  // Same key as the grid below — one cache entry, two subscribers.
  const ni = useIssuesCrossListInfinite(NEW_ISSUES_FILTERS, {
    enabled: isNewIssues,
  });
  // Same key as the grid below — one cache entry, two subscribers.
  const sim = useSimilarRailInfinite({ enabled: isSimilar });
  const similarSeed = sim.data?.pages[0]?.seed ?? null;
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  const itemCount = isContinueReading
    ? (cr.data?.items.length ?? null)
    : isOnDeck
      ? (od.data?.items.length ?? null)
      : isNewIssues
        ? // `total` rides only the first page (cursor-pagination
          // convention) — the full library-wide issue count.
          (ni.data?.pages[0]?.total ?? null)
        : isSimilar
          ? (sim.data?.pages[0]?.total ?? null)
          : isRecentHistory
            ? (rh.data?.items.length ?? null)
            : isRandomPicks
              ? (rp.data?.items.length ?? null)
              : isOnThisDay
                ? (otd.data?.items.length ?? null)
                : null;

  return (
    <div className="space-y-6">
      {/* Breadcrumb + eyebrow are desktop-only: on a phone the title
       *  already says where we are and every row above the grid costs
       *  a card row. */}
      <nav
        aria-label="Breadcrumb"
        className="text-muted-foreground flex items-center gap-1.5 text-xs max-sm:hidden"
      >
        <Link
          href="/"
          className="hover:text-foreground underline-offset-2 hover:underline"
        >
          Library
        </Link>
        <ChevronRight className="h-3 w-3" />
        <span className="text-foreground/80">{view.name}</span>
      </nav>

      <header className="flex flex-wrap items-end justify-between gap-4">
        <div className="min-w-0">
          <div className="text-muted-foreground inline-flex items-center gap-1.5 text-xs font-medium tracking-wider uppercase max-sm:hidden">
            <Sparkles className="h-3 w-3" />
            System rail
          </div>
          <h1 className="mt-1 text-2xl font-semibold tracking-tight max-sm:mt-0 sm:text-3xl">
            {view.name}
          </h1>
          {view.description && (
            <p className="text-muted-foreground mt-2 max-w-prose text-sm max-sm:line-clamp-2 max-sm:text-xs">
              {view.description}
            </p>
          )}
          {isSimilar && similarSeed ? (
            <p className="text-muted-foreground mt-2 text-sm">
              Because you read{" "}
              <Link
                href={seriesUrl(similarSeed)}
                className="text-foreground underline-offset-2 hover:underline"
              >
                {similarSeed.name}
              </Link>
            </p>
          ) : null}
          {itemCount != null && (
            <p className="text-muted-foreground mt-2 text-sm">
              {itemCount === 0
                ? "Nothing here yet."
                : `${itemCount} ${itemCount === 1 ? "item" : "items"}`}
            </p>
          )}
        </div>
        {/* Desktop: the card-size popover. Phones: the same fields in a
         *  bottom sheet behind a single icon, inline in the header row
         *  (one row, no sticky band — this page has no search). */}
        <div className="max-sm:hidden">
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
        </div>
        <PhoneControlsBar
          sticky={false}
          menuIcon={<LayoutGrid className="size-4" />}
          menu={() => (
            <CardSizeFields
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
              fieldId="phone-card-size"
            />
          )}
        />
      </header>

      <div
        className="grid gap-4"
        style={{
          gridTemplateColumns: `repeat(auto-fill, minmax(min(${cardSize}px, calc(50% - 0.5rem)), 1fr))`,
        }}
      >
        {isContinueReading ? (
          <ContinueReadingGrid loading={cr.isLoading} data={cr.data} />
        ) : isOnDeck ? (
          <OnDeckGrid loading={od.isLoading} data={od.data} />
        ) : isNewIssues ? (
          <NewIssuesGrid />
        ) : isSimilar ? (
          <SimilarSeriesGrid />
        ) : isRecentHistory ? (
          <ContinueReadingGrid
            loading={rh.isLoading}
            data={rh.data}
            emptyMessage="Nothing read yet. Issues you open (or finish) show up here, most recent first."
            primaryAction="details"
          />
        ) : isRandomPicks ? (
          <RandomPicksGrid loading={rp.isLoading} data={rp.data} />
        ) : isOnThisDay ? (
          <OnThisDayGrid loading={otd.isLoading} data={otd.data} />
        ) : null}
      </div>
    </div>
  );
}

/** Complete ingest-ordered issue list, newest first. Walks `/issues`
 *  pages through an IntersectionObserver sentinel (same idiom as
 *  `IssuesPanel`) — unlike the home rail, no per-series cap applies
 *  here, so a bulk import is fully browsable. */
function NewIssuesGrid() {
  const query = useIssuesCrossListInfinite(NEW_ISSUES_FILTERS);
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);

  // Depend on the three fields, not the whole result object — TanStack
  // returns a fresh object identity per render, so `[query]` would tear
  // the observer down and rebuild it on every state change.
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) {
            void fetchNextPage();
          }
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  if (query.isLoading) {
    return (
      <>
        {Array.from({ length: 12 }).map((_, i) => (
          <IssueCardSkeleton key={i} />
        ))}
      </>
    );
  }
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground col-span-full text-sm">
        Nothing here yet. Issues land in this list as library scans ingest them.
      </p>
    );
  }
  return (
    <>
      {items.map((issue) => (
        <IssueCard key={issue.id} issue={issue} />
      ))}
      <div ref={sentinelRef} aria-hidden className="col-span-full h-px" />
      {isFetchingNextPage
        ? Array.from({ length: 6 }).map((_, i) => (
            <IssueCardSkeleton key={`next-${i}`} />
          ))
        : null}
    </>
  );
}

/** WP-7.4: every visible neighbour of the rail's seed series, walked
 *  page by page through the IntersectionObserver sentinel. */
function SimilarSeriesGrid() {
  const query = useSimilarRailInfinite();
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) {
            void fetchNextPage();
          }
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  if (query.isLoading) {
    return (
      <>
        {Array.from({ length: 12 }).map((_, i) => (
          <SeriesCardSkeleton key={i} />
        ))}
      </>
    );
  }
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground col-span-full text-sm">
        Nothing to suggest yet. Read something and series that share its
        creators, characters, or story arcs show up here.
      </p>
    );
  }
  return (
    <>
      {items.map((item) => (
        <SimilarSeriesCard key={item.series.id} item={item} />
      ))}
      <div ref={sentinelRef} aria-hidden className="col-span-full h-px" />
      {isFetchingNextPage
        ? Array.from({ length: 6 }).map((_, i) => (
            <SeriesCardSkeleton key={`next-${i}`} />
          ))
        : null}
    </>
  );
}

function RandomPicksGrid({
  loading,
  data,
}: {
  loading: boolean;
  data: { items: SeriesView[] } | undefined;
}) {
  if (loading) {
    return (
      <>
        {Array.from({ length: 12 }).map((_, i) => (
          <SeriesCardSkeleton key={i} />
        ))}
      </>
    );
  }
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground col-span-full text-sm">
        Nothing left to pick from — every series in your library has been
        started.
      </p>
    );
  }
  return (
    <>
      {items.map((series) => (
        <SeriesCard key={series.id} series={series} size="md" />
      ))}
    </>
  );
}

function OnThisDayGrid({
  loading,
  data,
}: {
  loading: boolean;
  data: { items: IssueSummaryView[] } | undefined;
}) {
  if (loading) {
    return (
      <>
        {Array.from({ length: 12 }).map((_, i) => (
          <IssueCardSkeleton key={i} />
        ))}
      </>
    );
  }
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground col-span-full text-sm">
        No issues in your library were released on today&apos;s date. Only
        issues with a full release date (month and day) qualify.
      </p>
    );
  }
  return (
    <>
      {items.map((issue) => (
        <IssueCard key={issue.id} issue={issue} />
      ))}
    </>
  );
}

function ContinueReadingGrid({
  loading,
  data,
  emptyMessage,
  primaryAction,
}: {
  loading: boolean;
  data: { items: ContinueReadingCard[] } | undefined;
  emptyMessage?: string;
  primaryAction?: React.ComponentProps<
    typeof ProgressIssueCard
  >["primaryAction"];
}) {
  if (loading) {
    return (
      <>
        {Array.from({ length: 12 }).map((_, i) => (
          <ProgressIssueCardSkeleton key={i} />
        ))}
      </>
    );
  }
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground col-span-full text-sm">
        {emptyMessage ??
          "Nothing in progress right now. Issues you start (and don't finish) will appear here automatically."}
      </p>
    );
  }
  return (
    <>
      {items.map((card) => (
        <ProgressIssueCard
          key={card.issue.id}
          card={card}
          primaryAction={primaryAction}
        />
      ))}
    </>
  );
}

function OnDeckGrid({
  loading,
  data,
}: {
  loading: boolean;
  data: { items: OnDeckCardData[] } | undefined;
}) {
  if (loading) {
    return (
      <>
        {Array.from({ length: 12 }).map((_, i) => (
          <OnDeckCardSkeleton key={i} />
        ))}
      </>
    );
  }
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground col-span-full text-sm">
        Nothing queued up. Finish an issue and the next one in its series (or in
        a list you&apos;re working through) lands here.
      </p>
    );
  }
  return (
    <>
      {items.map((card, i) => (
        <OnDeckCard
          key={
            card.kind === "cbl_next"
              ? `cbl:${card.cbl_list_id}:${i}`
              : `series:${card.issue.series_id}:${i}`
          }
          card={card}
        />
      ))}
    </>
  );
}
