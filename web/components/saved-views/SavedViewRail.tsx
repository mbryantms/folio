"use client";

import * as React from "react";
import Link from "next/link";

import { CblStatsPills } from "@/components/cbl/CblStatsPills";
import { CblWindowCard } from "@/components/cbl/CblWindowCard";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { RailIconPicker } from "@/components/library/RailIconPicker";
import {
  useCblListWindow,
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
}: {
  view: SavedViewView;
  cardSize: number;
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
      <HorizontalScrollRail
        viewAllHref={railContent.hasMore ? viewDetailHref(view) : undefined}
        itemWidthPx={cardSize}
      >
        {railContent.body}
      </HorizontalScrollRail>
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
      <Link
        href={href}
        className="hover:text-foreground min-w-0 truncate text-lg font-semibold tracking-tight"
        title={view.name}
      >
        {view.name}
      </Link>
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
  void view;
  // Reading-window query: 3 finished entries on the left for context,
  // a wider upcoming tail on the right so users can browse several
  // issues ahead without bouncing to the detail page. The window
  // endpoint already filters out unmatched entries + library-ACL
  // gaps server-side.
  const window = useCblListWindow(listId, { before: 3, after: 24 });
  if (window.isLoading) {
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
  const data = window.data;
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <RailEmptyState
        message={
          (data?.total_matched ?? 0) === 0
            ? "No matched entries in this list."
            : "Nothing to show in this window."
        }
      />
    );
  }
  const currentIndex = data?.current_index ?? null;
  return (
    <>
      {items.map((entry, i) => {
        const isCurrent = i === currentIndex;
        return (
          <div
            key={entry.issue.id}
            style={itemStyle}
            className="shrink-0"
            // The rail wrapper looks for this attribute on mount to
            // auto-scroll the current entry into view.
            data-rail-current={isCurrent ? "true" : undefined}
          >
            <CblWindowCard entry={entry} isCurrent={isCurrent} />
          </div>
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
  return (
    <div className="border-border/60 text-muted-foreground rounded-md border border-dashed px-4 py-6 text-sm">
      {message}
    </div>
  );
}
