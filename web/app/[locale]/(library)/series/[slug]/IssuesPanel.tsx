"use client";

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import {
  Check,
  Circle,
  FileCog,
  FolderPlus,
  Pencil,
  Sparkles,
} from "lucide-react";
import { toast } from "sonner";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import { BulkArchiveEditDialog } from "@/components/library/BulkArchiveEditDialog";
import {
  BulkMarkReadDialog,
  BULK_BACKFILL_PROMPT_THRESHOLD,
} from "@/components/library/BulkMarkReadDialog";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { EditMetadataDialog } from "@/components/library/EditMetadataDialog";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { VirtualizedCardGrid } from "@/components/library/VirtualizedCardGrid";
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
import { apiMutate } from "@/lib/api/mutations";
import {
  useBulkMarkProgress,
  useBulkMarkSeriesMatchingProgress,
} from "@/lib/api/mutations";
import { useMe, useSeriesIssuesInfinite } from "@/lib/api/queries";
import { ISSUE_TEXT_H } from "@/lib/library/grid-window";
import { useSelection } from "@/lib/selection/use-selection";
import type { IssueSort, SortOrder } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import {
  SpecialsExtrasSection,
  splitMainAndSpecials,
} from "./SpecialsExtrasSection";

/** Stable no-op for the windowed main-run grid: fetch is driven by the
 *  sentinel below (which also handles the empty-main-run edge), so the
 *  virtualizer windows only and never fetches. */
const NOOP = () => {};

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
  initialQuery = "",
}: {
  /** Slug of the parent series — drives the `/series/{slug}/issues` fetch. */
  seriesSlug: string;
  issueCount: number | null;
  /** URL-derived initial value for the search input. Lets refresh +
   *  share preserve the in-page search ("issue 3" filter) the user had
   *  typed. Server-rendered into the page via `?q=`. */
  initialQuery?: string;
}) {
  const [q, setQ] = useState(initialQuery);
  const [sort, setSort] = useState<IssueSort>("number");
  const [order, setOrder] = useState<SortOrder>("asc");
  const [debouncedQ, setDebouncedQ] = useState(initialQuery.trim());

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

  // Mirror the debounced query into the URL via `replaceState` so a
  // refresh restores the search without forcing a full RSC navigation
  // on every keystroke. Same pattern the `/search` page uses; ignored
  // by the App-Router cache since the URL doesn't re-key the route.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    if (debouncedQ.length > 0) url.searchParams.set("q", debouncedQ);
    else url.searchParams.delete("q");
    window.history.replaceState({}, "", url.toString());
  }, [debouncedQ]);

  const filters = debouncedQ
    ? { q: debouncedQ, limit: 60 }
    : { sort, order, limit: 60 };

  const query = useSeriesIssuesInfinite(seriesSlug, filters);
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  // Split main-run from specials/annuals/oneshots (see spec §6.5).
  // The grid renders the main run; specials/extras go into a sibling
  // section below that's hidden when empty.
  const { mainRun: mainRunItems, specials: specialItems } =
    splitMainAndSpecials(items);
  const sentinelRef = useRef<HTMLDivElement | null>(null);

  // Multi-select state — first surface to land for the
  // multi-select-bulk-actions plan. M1 wires Mark read / Mark
  // unread; M3 adds Add-to-collection; M4 will append Remove etc.
  const selection = useSelection(items);
  const bulkMark = useBulkMarkProgress();
  const bulkMarkMatching = useBulkMarkSeriesMatchingProgress(seriesSlug);
  const [allMatchingSelected, setAllMatchingSelected] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [editMetadataOpen, setEditMetadataOpen] = useState(false);
  const [archiveEditOpen, setArchiveEditOpen] = useState(false);
  const [markReadOpen, setMarkReadOpen] = useState(false);
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const matchingTotal =
    query.data?.pages[0]?.total ?? issueCount ?? items.length;
  const matchingQuery = debouncedQ || undefined;
  const clearSelection = useCallback(() => {
    setAllMatchingSelected(false);
    selection.clear();
  }, [selection]);
  const submitMarkRead = useCallback(
    (backfill: boolean) => {
      if (allMatchingSelected) {
        bulkMarkMatching.mutate(
          { finished: true, backfill, q: matchingQuery },
          {
            onSuccess: () => {
              clearSelection();
              setMarkReadOpen(false);
            },
          },
        );
        return;
      }
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      bulkMark.mutate(
        { issue_ids: ids, finished: true, backfill },
        {
          onSuccess: () => {
            clearSelection();
            setMarkReadOpen(false);
          },
        },
      );
    },
    [
      allMatchingSelected,
      bulkMark,
      bulkMarkMatching,
      clearSelection,
      matchingQuery,
      selection.selected,
    ],
  );
  const runBulk = useCallback(
    (finished: boolean) => {
      if (allMatchingSelected) {
        if (finished && matchingTotal >= BULK_BACKFILL_PROMPT_THRESHOLD) {
          setMarkReadOpen(true);
          return;
        }
        bulkMarkMatching.mutate(
          { finished, q: matchingQuery },
          { onSuccess: clearSelection },
        );
        return;
      }
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      // Mark-read at scale (>= threshold) is overwhelmingly catalog
      // maintenance, not active reading — prompt before writing so the
      // user can choose. Mark-unread + small mark-read selections
      // skip the prompt and proceed directly (no activity to mislead).
      if (finished && ids.length >= BULK_BACKFILL_PROMPT_THRESHOLD) {
        setMarkReadOpen(true);
        return;
      }
      bulkMark.mutate(
        { issue_ids: ids, finished },
        { onSuccess: clearSelection },
      );
    },
    [
      allMatchingSelected,
      bulkMark,
      bulkMarkMatching,
      clearSelection,
      matchingQuery,
      matchingTotal,
      selection.selected,
    ],
  );
  const selectedTargets = Array.from(selection.selected).map((id) => ({
    entry_kind: "issue" as const,
    ref_id: id,
  }));

  const [bulkFetchPending, setBulkFetchPending] = useState(false);
  const runBulkMetadataFetch = async () => {
    const ids = selection.selected;
    if (ids.size === 0) return;
    const slugs = items.filter((iss) => ids.has(iss.id)).map((iss) => iss.slug);
    if (slugs.length === 0) return;
    setBulkFetchPending(true);
    const toastId = toast.loading(
      `Searching providers for ${slugs.length} issue${slugs.length === 1 ? "" : "s"}…`,
    );
    // POST only enqueues the search job — returns ~immediately. Run in
    // parallel; the server's per-provider token bucket gates the actual
    // upstream calls. Surface a single summary toast at the end.
    const results = await Promise.allSettled(
      slugs.map((slug) =>
        apiMutate({
          path: `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(slug)}/metadata/search`,
          method: "POST",
        }),
      ),
    );
    const ok = results.filter((r) => r.status === "fulfilled").length;
    const failed = results.length - ok;
    if (failed === 0) {
      toast.success(
        `Queued metadata search for ${ok} issue${ok === 1 ? "" : "s"}.`,
        {
          id: toastId,
        },
      );
    } else if (ok === 0) {
      toast.error(
        `Failed to queue any metadata searches (${failed} error${failed === 1 ? "" : "s"}).`,
        {
          id: toastId,
        },
      );
    } else {
      toast.message(`Queued ${ok}, ${failed} failed.`, { id: toastId });
    }
    setBulkFetchPending(false);
    clearSelection();
  };

  // Auto-fetch the next page when the sentinel scrolls into view.
  // Depend on the three fields, not the whole result object — TanStack
  // returns a fresh object identity per render, so `[query]` tore the
  // observer down and rebuilt it on every state change.
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  useEffect(() => {
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
        setAllMatchingSelected(false);
        selection.exit();
      } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setAllMatchingSelected(false);
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
  } as CSSProperties;

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
            aria-label="Search issues in this series"
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
          <SelectModeButton
            ref={selectButtonRef}
            active={selection.selectMode}
            onEnter={() => selection.enter()}
            onExit={() => selection.exit()}
          />
        </div>
      </div>

      <SelectionToolbar
        open={selection.selectMode}
        count={allMatchingSelected ? matchingTotal : selection.count}
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
            disabled: allMatchingSelected,
          },
        ]}
        actionGroups={[
          {
            id: "editing",
            label: "Editing",
            icon: Pencil,
            actions: [
              {
                id: "edit-metadata",
                label: "Edit metadata…",
                icon: Pencil,
                onClick: () => setEditMetadataOpen(true),
                disabled: allMatchingSelected,
              },
              {
                id: "fetch-metadata",
                label: "Fetch metadata",
                icon: Sparkles,
                onClick: () => void runBulkMetadataFetch(),
                disabled: allMatchingSelected || bulkFetchPending,
              },
              // Admin-only: rewrites archive files. The server skips issues
              // whose library has writeback off (reported in the result).
              ...(isAdmin
                ? [
                    {
                      id: "edit-archives",
                      label: "Edit archives…",
                      icon: FileCog,
                      onClick: () => setArchiveEditOpen(true),
                      disabled: allMatchingSelected,
                    },
                  ]
                : []),
            ],
          },
        ]}
        onDone={() => {
          setAllMatchingSelected(false);
          selection.exit();
        }}
        onClear={clearSelection}
        onSelectAll={() => {
          setAllMatchingSelected(false);
          selection.selectAll();
        }}
        onSelectAllMatching={
          allMatchingSelected
            ? undefined
            : () => {
                selection.clear();
                setAllMatchingSelected(true);
              }
        }
        matchingTotal={matchingTotal}
        isPending={bulkMark.isPending || bulkMarkMatching.isPending}
      />
      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) clearSelection();
        }}
        targets={selectedTargets}
      />
      <EditMetadataDialog
        open={editMetadataOpen}
        onOpenChange={(next) => {
          setEditMetadataOpen(next);
          if (!next) clearSelection();
        }}
        issueIds={Array.from(selection.selected)}
      />
      <BulkArchiveEditDialog
        open={archiveEditOpen}
        onOpenChange={(next) => {
          setArchiveEditOpen(next);
          if (!next) clearSelection();
        }}
        issueIds={Array.from(selection.selected)}
      />
      <BulkMarkReadDialog
        open={markReadOpen}
        onOpenChange={setMarkReadOpen}
        count={allMatchingSelected ? matchingTotal : selection.selected.size}
        onConfirm={submitMarkRead}
        isPending={bulkMark.isPending || bulkMarkMatching.isPending}
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
      ) : mainRunItems.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          {debouncedQ
            ? `No main-run issues matched "${debouncedQ}".`
            : "No main-run issues yet — see Specials & Extras below."}
        </p>
      ) : (
        // Window-virtualize the main run (audit G1, parity with the
        // library grid). Fetch stays on the sentinel below — it covers
        // the edge where a loaded page is all specials and `mainRun` is
        // momentarily empty — so this grid only windows (hasNextPage
        // false). `splitMainAndSpecials` order is preserved.
        <VirtualizedCardGrid
          items={mainRunItems}
          cardSize={cardSize}
          estimateTextHeight={ISSUE_TEXT_H}
          hasNextPage={false}
          isFetchingNextPage={false}
          fetchNextPage={NOOP}
          renderCard={(item) => {
            const iss = item as (typeof mainRunItems)[number];
            return (
              <IssueCard
                issue={iss}
                selectMode={
                  selection.selectMode
                    ? {
                        isActive: true,
                        isSelected:
                          allMatchingSelected || selection.isSelected(iss.id),
                        onToggle: (ev) => {
                          if (allMatchingSelected) {
                            setAllMatchingSelected(false);
                            selection.clear();
                          }
                          selection.toggle(iss.id, ev);
                        },
                      }
                    : undefined
                }
                onEnterSelectMode={(id) => {
                  // Long-press → sheet → "Select": enter select
                  // mode AND pre-select the long-pressed card.
                  selection.toggle(id);
                }}
              />
            );
          }}
        />
      )}

      <div
        ref={sentinelRef}
        aria-hidden
        className={cn("h-12", query.hasNextPage ? "" : "hidden")}
      />
      {query.isFetchingNextPage && (
        <p role="status" className="text-muted-foreground mt-2 text-center text-xs">
          Loading more…
        </p>
      )}

      {!query.isLoading && specialItems.length > 0 && (
        <SpecialsExtrasSection items={specialItems} gridStyle={gridStyle} />
      )}
    </section>
  );
}

function IssueGridSkeleton({ style }: { style: CSSProperties }) {
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
