"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Circle, FolderPlus, ListChecks } from "lucide-react";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { useCardSize } from "@/components/library/use-card-size";
import { Input } from "@/components/ui/input";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import { useBulkMarkProgress } from "@/lib/api/mutations";
import { useSeriesIssuesInfinite } from "@/lib/api/queries";
import { useSelection } from "@/lib/selection/use-selection";
import type { IssueSort, SortOrder } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/** Per-series listing only supports a subset of `IssueSort` — the
 *  cross-library discovery sorts (`year`, `page_count`, `user_rating`)
 *  are rejected by `/series/{slug}/issues` server-side, so we don't
 *  surface them here either. */
const SORT_LABELS: Partial<Record<IssueSort, string>> = {
  number: "Issue number",
  created_at: "Date added",
  updated_at: "Date updated",
};

/**
 * Card-size bounds for the View → Card size slider. The grid uses
 * `repeat(auto-fill, minmax(<size>px, 1fr))` so column count adapts
 * fluidly as the user drags. Step matches a comic cover's natural
 * aspect ratio increments — finer steps just look like jitter.
 */
const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.series.cardSize";

export function IssuesPanel({
  seriesSlug,
  issueCount,
}: {
  /** Slug of the parent series — drives the `/series/{slug}/issues` fetch. */
  seriesSlug: string;
  issueCount: number | null;
}) {
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<IssueSort>("number");
  const [order, setOrder] = useState<SortOrder>("asc");
  const [debouncedQ, setDebouncedQ] = useState("");

  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  // Debounce search to avoid hammering the endpoint while the user types.
  useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(q.trim()), 200);
    return () => clearTimeout(t);
  }, [q]);

  const filters = debouncedQ
    ? { q: debouncedQ, limit: 60 }
    : { sort, order, limit: 60 };

  const query = useSeriesIssuesInfinite(seriesSlug, filters);
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  const sentinelRef = useRef<HTMLDivElement | null>(null);

  // Multi-select state — first surface to land for the
  // multi-select-bulk-actions plan. M1 wires Mark read / Mark
  // unread; M3 adds Add-to-collection; M4 will append Remove etc.
  const selection = useSelection(items);
  const bulkMark = useBulkMarkProgress();
  const [pickerOpen, setPickerOpen] = useState(false);
  const runBulk = useCallback(
    (finished: boolean) => {
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      bulkMark.mutate(
        { issue_ids: ids, finished },
        {
          onSuccess: () => {
            selection.clear();
          },
        },
      );
    },
    [bulkMark, selection],
  );
  const selectedTargets = Array.from(selection.selected).map((id) => ({
    entry_kind: "issue" as const,
    ref_id: id,
  }));

  // Auto-fetch the next page when the sentinel scrolls into view.
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (query.hasNextPage && !query.isFetchingNextPage) {
            void query.fetchNextPage();
          }
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [query]);

  // Esc exits select mode entirely; Cmd/Ctrl+A selects every loaded
  // card. Both gated on `selectMode` so other handlers stay free
  // when the toolbar isn't up. `shouldSkipHotkey` keeps these from
  // firing while the user is typing in the search input.
  const selectButtonRef = useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = useRef(false);
  useEffect(() => {
    if (!selection.selectMode) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        selection.exit();
      } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        selection.selectAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection]);
  // Restore focus to the trigger button when leaving select mode so
  // keyboard users land back where they were. Skips the initial
  // mount (no toolbar → no transition).
  useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);

  // CSS custom property drives the grid's `minmax`. Falling back to the
  // default px value keeps the grid sane during initial paint before the
  // localStorage rehydrate effect runs.
  const gridStyle = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  } as React.CSSProperties;

  return (
    <section className="mt-10">
      <div className="mb-4 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">Issues</h2>
          {issueCount != null && (
            <p className="text-muted-foreground text-xs">
              {issueCount} {issueCount === 1 ? "issue" : "issues"}
            </p>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            type="search"
            placeholder="Search issues…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            className="w-56"
          />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm" disabled={!!debouncedQ}>
                Sort: {SORT_LABELS[sort]} ({order === "asc" ? "↑" : "↓"})
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuLabel>Sort by</DropdownMenuLabel>
              <DropdownMenuRadioGroup
                value={sort}
                onValueChange={(v) => setSort(v as IssueSort)}
              >
                <DropdownMenuRadioItem value="number">
                  Issue number
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="created_at">
                  Date added
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="updated_at">
                  Date updated
                </DropdownMenuRadioItem>
              </DropdownMenuRadioGroup>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                onSelect={() => setOrder((o) => (o === "asc" ? "desc" : "asc"))}
              >
                Reverse order
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
          {!selection.selectMode && (
            <Button
              ref={selectButtonRef}
              variant="outline"
              size="sm"
              onClick={() => selection.enter()}
              aria-label="Enter select mode"
            >
              <ListChecks className="mr-1.5 h-4 w-4" />
              Select
            </Button>
          )}
        </div>
      </div>

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={items.length}
        primary={[
          {
            id: "mark-read",
            label: "Mark read",
            icon: Check,
            onClick: () => runBulk(true),
          },
          {
            id: "mark-unread",
            label: "Mark unread",
            icon: Circle,
            onClick: () => runBulk(false),
          },
        ]}
        overflow={[
          {
            id: "add-to-collection",
            label: "Add to collection…",
            icon: FolderPlus,
            onClick: () => setPickerOpen(true),
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkMark.isPending}
      />
      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) selection.clear();
        }}
        targets={selectedTargets}
      />

      {query.isError && (
        <p className="text-destructive text-sm">
          Failed to load issues. {String(query.error)}
        </p>
      )}

      {query.isLoading ? (
        <IssueGridSkeleton style={gridStyle} />
      ) : items.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          {debouncedQ ? `No issues matched "${debouncedQ}".` : "No issues yet."}
        </p>
      ) : (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {items.map((iss) => (
            <li key={iss.id}>
              <IssueCard
                issue={iss}
                selectMode={
                  selection.selectMode
                    ? {
                        isActive: true,
                        isSelected: selection.isSelected(iss.id),
                        onToggle: (ev) => selection.toggle(iss.id, ev),
                      }
                    : undefined
                }
                onEnterSelectMode={(id) => {
                  // Long-press → sheet → "Select": enter select
                  // mode AND pre-select the long-pressed card.
                  selection.toggle(id);
                }}
              />
            </li>
          ))}
        </ul>
      )}

      <div
        ref={sentinelRef}
        aria-hidden
        className={cn("h-12", query.hasNextPage ? "" : "hidden")}
      />
      {query.isFetchingNextPage && (
        <p className="text-muted-foreground mt-2 text-center text-xs">
          Loading more…
        </p>
      )}
    </section>
  );
}

function IssueGridSkeleton({ style }: { style: React.CSSProperties }) {
  return (
    <ul role="list" className="grid gap-4" style={style}>
      {Array.from({ length: 12 }).map((_, i) => (
        <li key={i}>
          <IssueCardSkeleton />
        </li>
      ))}
    </ul>
  );
}
