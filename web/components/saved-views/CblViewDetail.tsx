"use client";

import * as React from "react";
import { Check, Download, EyeOff, Loader2, RefreshCw } from "lucide-react";

import { CblDetail, CblInfoRow } from "@/components/cbl/cbl-detail";
import { CblIssueCard } from "@/components/cbl/cbl-issue-card";
import { CblStatsPills } from "@/components/cbl/CblStatsPills";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { useCardSize } from "@/components/library/use-card-size";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { PopoverPortalContainer } from "@/components/ui/popover";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useCblList, useCblListEntriesInfinite } from "@/lib/api/queries";
import { useRefreshCblList } from "@/lib/api/mutations";
import type { CblEntryHydratedView, SavedViewView } from "@/lib/api/types";
import { useCblHideMissing } from "@/lib/cbl/use-hide-missing";

import { ViewHeader } from "./ViewHeader";

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.savedView.cardSize";

/** Read-first detail page for a CBL saved view. Mirrors filter views:
 *  the page is a consumption surface (issues in CBL position order),
 *  with Edit / Pin/Unpin / Refresh / Export in the header. The full
 *  management UI (Reading order / Resolution / History / Settings
 *  tabs) lives behind the Edit button via a wide dialog. */
export function CblViewDetail({ savedView }: { savedView: SavedViewView }) {
  const listId = savedView.cbl_list_id;
  if (!listId) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Saved view is marked as CBL but has no `cbl_list_id`.
      </div>
    );
  }
  return <CblViewDetailInner savedView={savedView} listId={listId} />;
}

function CblViewDetailInner({
  savedView,
  listId,
}: {
  savedView: SavedViewView;
  listId: string;
}) {
  const detail = useCblList(listId);
  // Server-filter when `hideMissing` is on: only fetch entries we'll
  // render. Otherwise pull everything in position order. Entries
  // arrive with their `IssueSummaryView` already attached, so no
  // separate hydration round-trip is needed (the old
  // `useCblListIssues({ limit: 1000 })` is gone).
  const [hideMissing, setHideMissing] = useCblHideMissing(listId);
  const entriesQuery = useCblListEntriesInfinite(listId, {
    status: hideMissing ? "matched,ambiguous,manual" : undefined,
  });
  const refresh = useRefreshCblList(listId);
  const [editOpen, setEditOpen] = React.useState(false);
  // Re-anchors `ManualMatchPopover` (and any other descendant popover)
  // into the SheetContent subtree. Without this they portal to
  // document.body, where the Sheet's modal aria-hide makes the search
  // input render but reject focus/clicks.
  const [editPortalContainer, setEditPortalContainer] =
    React.useState<HTMLElement | null>(null);
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  // Infinite-scroll sentinel — once it intersects the viewport,
  // fetch the next page. Matches the pattern in IssuesPanel +
  // ResolutionTab so behavior stays consistent across surfaces.
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (
            entriesQuery.hasNextPage &&
            !entriesQuery.isFetchingNextPage
          ) {
            void entriesQuery.fetchNextPage();
          }
        }
      },
      { rootMargin: "600px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [entriesQuery]);

  if (detail.isLoading) {
    return (
      <div className="text-muted-foreground py-12 text-sm">Loading view…</div>
    );
  }
  if (detail.isError || !detail.data) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Failed to load view.
      </div>
    );
  }

  const list = detail.data;
  const loadedEntries: CblEntryHydratedView[] =
    entriesQuery.data?.pages.flatMap((p) => p.items) ?? [];
  const filterTotal = entriesQuery.data?.pages[0]?.total ?? null;
  const missingCount = list.stats.missing;
  const canRefresh = list.source_kind !== "upload";
  const gridStyle: React.CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };

  // Build the render plan. When `hideMissing` is on, the server has
  // already filtered out missing entries; we still walk loaded
  // positions to insert a `gap` placeholder where the canonical CBL
  // index isn't contiguous. Position numbers on the visible cards
  // stay truthful regardless of how many were hidden.
  type RenderItem =
    | { kind: "entry"; entry: CblEntryHydratedView }
    | { kind: "gap"; key: string; count: number };
  const items: RenderItem[] = [];
  if (hideMissing) {
    let prevPos: number | null = null;
    for (const entry of loadedEntries) {
      if (prevPos !== null) {
        const gap = entry.position - prevPos - 1;
        if (gap > 0) {
          items.push({
            kind: "gap",
            key: `gap-${prevPos}-${entry.position}`,
            count: gap,
          });
        }
      }
      items.push({ kind: "entry", entry });
      prevPos = entry.position;
    }
  } else {
    for (const entry of loadedEntries) {
      items.push({ kind: "entry", entry });
    }
  }

  return (
    <div className="space-y-6">
      <ViewHeader
        view={savedView}
        onEdit={() => setEditOpen(true)}
        titleSuffix={renderYearRangeBadge(
          savedView.custom_year_start,
          savedView.custom_year_end,
        )}
        extraActions={
          <>
            {/* Same two-pill summary the home rail header carries —
             *  `size="header"` bumps padding/typography so the pills
             *  line up with the adjacent `size="sm"` icon button. */}
            <CblStatsPills cblListId={list.id} size="header" />
            <CardSizeOptions
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
            />
          </>
        }
        extraMenuItems={
          <>
            <DropdownMenuItem asChild>
              <a
                href={`/api/me/cbl-lists/${list.id}/export`}
                download
                title="Download as .cbl"
              >
                <Download className="mr-2 h-4 w-4" /> Export
              </a>
            </DropdownMenuItem>
            {missingCount > 0 ? (
              <DropdownMenuItem
                onSelect={(e) => {
                  e.preventDefault();
                  setHideMissing(!hideMissing);
                }}
              >
                {hideMissing ? (
                  <Check className="mr-2 h-4 w-4" />
                ) : (
                  <EyeOff className="mr-2 h-4 w-4" />
                )}
                {hideMissing
                  ? `Showing matched only (${missingCount} hidden)`
                  : `Hide ${missingCount} missing`}
              </DropdownMenuItem>
            ) : null}
            {canRefresh ? (
              <DropdownMenuItem
                onSelect={(e) => {
                  e.preventDefault();
                  if (!refresh.isPending) refresh.mutate({});
                }}
                disabled={refresh.isPending}
              >
                <RefreshCw
                  className={`mr-2 h-4 w-4 ${refresh.isPending ? "animate-spin" : ""}`}
                />
                Refresh
              </DropdownMenuItem>
            ) : null}
          </>
        }
      />
      {/* Catalog/source path + import date — useful context on the
       *  desktop detail page, but redundant on mobile where the title
       *  already identifies the list. Hide to save vertical real
       *  estate; full info still lives on the CBL management page
       *  (`<CblDetail>`) which renders the same row at all widths. */}
      {/* Year range moved up next to the title via `titleSuffix`; this
       *  row keeps source + matchers + imported date. */}
      <div className="hidden md:block">
        <CblInfoRow list={list} />
      </div>
      {entriesQuery.isLoading ? (
        <div className="text-muted-foreground py-12 text-sm">
          Loading entries…
        </div>
      ) : list.stats.total === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
          This list has no entries yet.
        </div>
      ) : items.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
          Every entry in this list is currently missing from your library.
          Toggle &ldquo;Hide missing&rdquo; off to see them.
        </div>
      ) : (
        <>
          <ul role="list" className="grid gap-3" style={gridStyle}>
            {items.map((item) =>
              item.kind === "entry" ? (
                <li key={item.entry.id}>
                  <CblIssueCard
                    entry={item.entry}
                    issue={item.entry.issue ?? undefined}
                    cblSavedViewId={savedView.id}
                  />
                </li>
              ) : (
                <li
                  key={item.key}
                  className="grid place-items-center"
                  aria-label={`${item.count} missing ${item.count === 1 ? "entry" : "entries"} hidden`}
                  title={`${item.count} missing ${item.count === 1 ? "entry" : "entries"} hidden`}
                >
                  <div className="border-border/60 text-muted-foreground/80 inline-flex flex-col items-center rounded-md border border-dashed px-2.5 py-1.5">
                    <span className="font-mono text-sm leading-none tracking-widest">
                      •••
                    </span>
                    <span className="mt-1 text-[10px] tracking-wider uppercase">
                      {item.count} missing
                    </span>
                  </div>
                </li>
              ),
            )}
          </ul>
          <div
            ref={sentinelRef}
            aria-hidden
            className={entriesQuery.hasNextPage ? "h-12" : "hidden"}
          />
          {entriesQuery.isFetchingNextPage ? (
            <p className="text-muted-foreground flex items-center justify-center gap-2 text-xs">
              <Loader2 className="h-3 w-3 animate-spin" /> Loading more (
              {loadedEntries.length}
              {filterTotal != null ? ` of ${filterTotal}` : ""})…
            </p>
          ) : null}
        </>
      )}

      <Sheet open={editOpen} onOpenChange={setEditOpen}>
        <SheetContent
          ref={setEditPortalContainer}
          side="right"
          // Wider than the filter-view sheet — the Reading-order tab
          // hosts a 6-column virtualized table that wants more
          // horizontal room than the filter builder does. `p-0` so
          // the header gets its own divider and the body owns its
          // padding. `overflow-visible` so the manual-match popover
          // (portaled into this content via PopoverPortalContainer)
          // can extend past the sheet's edge when collision detection
          // flips it outward; tab bodies own their own scroll.
          className="flex w-full flex-col gap-0 overflow-visible p-0 sm:max-w-3xl lg:max-w-4xl xl:max-w-5xl"
        >
          <SheetHeader className="border-border/60 border-b px-6 py-4 pr-12">
            <SheetTitle>Manage {savedView.name}</SheetTitle>
            <SheetDescription>
              Resolve missing or ambiguous matches, review refresh history,
              tweak metadata.
            </SheetDescription>
          </SheetHeader>
          <PopoverPortalContainer value={editPortalContainer}>
            <div className="flex min-h-0 flex-1 flex-col px-6 py-4">
              <CblDetail savedView={savedView} />
            </div>
          </PopoverPortalContainer>
        </SheetContent>
      </Sheet>
    </div>
  );
}

/** Compact year-range string for the title-adjacent header slot.
 *  `2002–2026` when both bounds exist, single-sided (`from 2002`,
 *  `up to 2026`) when only one does, equal-year collapse, `null`
 *  when neither is set. */
function formatYearRange(
  start?: number | null,
  end?: number | null,
): string | null {
  if (start != null && end != null) {
    return start === end ? `${start}` : `${start}–${end}`;
  }
  if (start != null) return `from ${start}`;
  if (end != null) return `up to ${end}`;
  return null;
}

/** Year-range badge for `ViewHeader.titleSuffix`. Renders the formatted
 *  range as a tooltip-trigger so the user can hover for a quick
 *  description of what the range is and where it came from. Returns
 *  `null` when the saved view has no year bounds — keeps the header
 *  tight for the common no-overlay case. */
function renderYearRangeBadge(
  start?: number | null,
  end?: number | null,
): React.ReactNode {
  const label = formatYearRange(start, end);
  if (!label) return null;
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className="cursor-help underline decoration-dotted decoration-1 underline-offset-4"
            aria-label={`Year range: ${label}`}
          >
            {label}
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom" className="max-w-xs">
          Year range covered by this list. Auto-filled from the earliest
          and latest entry at import; edit in Settings.
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
