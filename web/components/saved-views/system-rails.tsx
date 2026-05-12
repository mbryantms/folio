"use client";

import * as React from "react";

import {
  OnDeckCard,
  OnDeckCardSkeleton,
} from "@/components/library/OnDeckCard";
import {
  ProgressIssueCard,
  ProgressIssueCardSkeleton,
} from "@/components/library/ProgressIssueCard";
import type { OnDeckCard as OnDeckCardData } from "@/lib/api/types";
import { useContinueReading, useOnDeck } from "@/lib/api/queries";

/**
 * Continue-reading rail body. Lists in-progress issues most-recent first,
 * with a thin progress bar overlay + inline kebab actions on each card.
 *
 * Hides itself entirely when empty (returns `null`) so the empty state
 * doesn't take up a row above the user's saved views.
 */
export function ContinueReadingRailBody({
  itemStyle,
}: {
  itemStyle: React.CSSProperties;
}) {
  const q = useContinueReading();
  if (q.isLoading) {
    return (
      <>
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} style={itemStyle} className="shrink-0">
            <ProgressIssueCardSkeleton />
          </div>
        ))}
      </>
    );
  }
  const items = q.data?.items ?? [];
  if (items.length === 0) {
    // Parent suppresses the whole rail when there's nothing to show —
    // see `SavedViewRail.tsx`'s system-kind branch.
    return null;
  }
  return (
    <>
      {items.map((card) => (
        <div key={card.issue.id} style={itemStyle} className="shrink-0">
          <ProgressIssueCard card={card} />
        </div>
      ))}
    </>
  );
}

/**
 * On-deck rail body. Lists `series_next` and `cbl_next` cards interleaved
 * by recency. Empty when the user has no progress yet OR every started
 * series/CBL has been caught up.
 */
export function OnDeckRailBody({
  itemStyle,
}: {
  itemStyle: React.CSSProperties;
}) {
  const q = useOnDeck();
  if (q.isLoading) {
    return (
      <>
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} style={itemStyle} className="shrink-0">
            <OnDeckCardSkeleton />
          </div>
        ))}
      </>
    );
  }
  const items = q.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <>
      {items.map((card, i) => (
        <div key={cardKey(card, i)} style={itemStyle} className="shrink-0">
          <OnDeckCard card={card} />
        </div>
      ))}
    </>
  );
}

/** Stable React key for an On Deck card. The card kind decides which id
 *  uniquely identifies the row (series_id vs cbl_list_id) — the issue id
 *  alone isn't enough because the same issue can appear under both kinds
 *  in edge cases (a CBL whose next entry IS the next-up issue in its
 *  series). Index falls in as a tiebreaker. */
function cardKey(card: OnDeckCardData, index: number): string {
  if (card.kind === "series_next") {
    return `series:${card.issue.series_id}:${index}`;
  }
  return `cbl:${card.cbl_list_id}:${index}`;
}

/**
 * True when a system rail of the given key has no items to render and the
 * parent should suppress the entire section (header + scroller). Used by
 * `SavedViewRail` to avoid an empty "Continue reading" header on a fresh
 * account.
 *
 * This is a separate hook so the parent can call it WITHOUT mounting the
 * rail body — keeping the rail body responsible for skeletons + cards
 * without coupling it to layout decisions.
 */
export function useSystemRailIsEmpty(systemKey: string): boolean {
  const cr = useContinueReading();
  const od = useOnDeck();
  if (systemKey === "continue_reading") {
    return !cr.isLoading && (cr.data?.items.length ?? 0) === 0;
  }
  if (systemKey === "on_deck") {
    return !od.isLoading && (od.data?.items.length ?? 0) === 0;
  }
  return false;
}
