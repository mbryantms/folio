"use client";

import * as React from "react";
import Link from "next/link";
import { ChevronRight, Sparkles } from "lucide-react";

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
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import {
  useContinueReading,
  useIssuesCrossListInfinite,
  useOnDeck,
} from "@/lib/api/queries";
import type {
  ContinueReadingCard,
  OnDeckCard as OnDeckCardData,
  SavedViewView,
} from "@/lib/api/types";

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

  const cr = useContinueReading({ enabled: isContinueReading });
  const od = useOnDeck({ enabled: isOnDeck });
  // Same key as the grid below — one cache entry, two subscribers.
  const ni = useIssuesCrossListInfinite(NEW_ISSUES_FILTERS, {
    enabled: isNewIssues,
  });
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
        : null;

  return (
    <div className="space-y-6">
      <nav
        aria-label="Breadcrumb"
        className="text-muted-foreground flex items-center gap-1.5 text-xs"
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
          <div className="text-muted-foreground inline-flex items-center gap-1.5 text-xs font-medium tracking-wider uppercase">
            <Sparkles className="h-3 w-3" />
            System rail
          </div>
          <h1 className="mt-1 text-2xl font-semibold tracking-tight sm:text-3xl">
            {view.name}
          </h1>
          {view.description && (
            <p className="text-muted-foreground mt-2 max-w-prose text-sm">
              {view.description}
            </p>
          )}
          {itemCount != null && (
            <p className="text-muted-foreground mt-2 text-sm">
              {itemCount === 0
                ? "Nothing here yet."
                : `${itemCount} ${itemCount === 1 ? "item" : "items"}`}
            </p>
          )}
        </div>
        <CardSizeOptions
          cardSize={cardSize}
          onCardSize={setCardSize}
          min={CARD_SIZE_MIN}
          max={CARD_SIZE_MAX}
          step={CARD_SIZE_STEP}
          defaultSize={CARD_SIZE_DEFAULT}
        />
      </header>

      <div
        className="grid gap-4"
        style={{
          gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
        }}
      >
        {isContinueReading ? (
          <ContinueReadingGrid loading={cr.isLoading} data={cr.data} />
        ) : isOnDeck ? (
          <OnDeckGrid loading={od.isLoading} data={od.data} />
        ) : isNewIssues ? (
          <NewIssuesGrid />
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

function ContinueReadingGrid({
  loading,
  data,
}: {
  loading: boolean;
  data: { items: ContinueReadingCard[] } | undefined;
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
        Nothing in progress right now. Issues you start (and don&apos;t finish)
        will appear here automatically.
      </p>
    );
  }
  return (
    <>
      {items.map((card) => (
        <ProgressIssueCard key={card.issue.id} card={card} />
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
