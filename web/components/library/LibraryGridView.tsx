"use client";

import * as React from "react";
import { Check, Circle, FolderPlus } from "lucide-react";
import { toast } from "sonner";

import { ActiveChips } from "@/components/library/ActiveChips";
import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import {
  BulkMarkReadDialog,
  BULK_BACKFILL_PROMPT_THRESHOLD,
} from "@/components/library/BulkMarkReadDialog";
import { AtoZJumpRail } from "@/components/library/AtoZJumpRail";
import { FilterSheet } from "@/components/library/FilterSheet";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { LibraryGridToolbar } from "@/components/library/LibraryGridToolbar";
import { MetadataWorklistButton } from "@/components/library/MetadataWorklistButton";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { VirtualizedCardGrid } from "@/components/library/VirtualizedCardGrid";
import { ISSUE_TEXT_H, SERIES_TEXT_H } from "@/lib/library/grid-window";
import type { FilterBuilderState } from "@/components/filters/filter-builder";
import type {
  LibraryGridInitialFilters,
  LibraryGridMode,
} from "@/components/library/library-grid-filters";
import { libraryGridStateToFilterBuilderState } from "@/components/library/libraryGridStateToFilterState";
import { NewFilterViewDialog } from "@/components/saved-views/AddViewButton";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { useCardSize } from "@/components/library/use-card-size";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import {
  useBulkMarkProgress,
  useBulkMarkSeriesProgress,
} from "@/lib/api/mutations";
import {
  useIssuesCrossListInfinite,
  useSeriesListInfinite,
} from "@/lib/api/queries";
import { useLibraryGridFilters } from "@/lib/library/use-grid-filters";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";
import { useCoarsePointerActionsHint } from "@/lib/ui/use-coarse-pointer";

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.libraryGrid.cardSize";

/** Library grid: paginated series listing with metadata-driven
 *  filters in a right-side Sheet drawer. Default sort is alphabetical
 *  (name asc) so a fresh library landing matches user expectation;
 *  the sort dropdown is the escape hatch for "recently added /
 *  updated" workflows.
 *
 *  When `libraryId` is null the grid spans every library the user can
 *  see (via the server's ACL-enforced `/series` listing). Otherwise it
 *  scopes to that single library — and forwards `library` to the
 *  metadata-suite endpoints so facet menus only show values that
 *  exist *inside that library*.
 *
 *  Refactored in audit-remediation M7.3 (1206 → ~380 LOC): filter
 *  state lives in `useLibraryGridFilters`; the toolbar, FilterSheet,
 *  and ActiveChips are their own files. This component composes
 *  them with the grid + total/empty/loading states.
 */
export function LibraryGridView({
  libraryId,
  libraryName,
  libraryCount,
  initialFilters,
}: {
  libraryId: string | null;
  libraryName: string;
  /** When `libraryId` is null, the count of libraries the user can
   *  access — surfaced in the subtitle so "All Libraries" feels less
   *  abstract. */
  libraryCount?: number;
  initialFilters?: LibraryGridInitialFilters;
}) {
  // One-time touch hint for the now-persistent cover kebab (audit B16).
  useCoarsePointerActionsHint();
  const filters = useLibraryGridFilters(libraryId, initialFilters);
  const {
    mode,
    setMode,
    q,
    setQ,
    trimmedQ,
    seriesSort,
    setSeriesSort,
    issueSort,
    setIssueSort,
    order,
    setOrder,
    status,
    setStatus,
    metadataCompleteness,
    setMetadataCompleteness,
    readStatus,
    setReadStatus,
    startsWith,
    setStartsWith,
    yearFrom,
    setYearFrom,
    yearTo,
    setYearTo,
    publishers,
    setPublishers,
    languages,
    setLanguages,
    ageRatings,
    setAgeRatings,
    genres,
    setGenres,
    tags,
    setTags,
    credits,
    setCreditRole,
    anyCredits,
    setAnyCredits,
    characters,
    setCharacters,
    teams,
    setTeams,
    locations,
    setLocations,
    ratingRange,
    setRatingRange,
    facetCount,
    seriesFilters,
    issueFilters,
    clearFacets,
  } = filters;

  const [filterOpen, setFilterOpen] = React.useState(false);
  // M2 of saved-views parity — "Save as view…" dialog state. Seeded
  // from the current facet snapshot at click time; cleared when the
  // dialog closes.
  const [saveViewOpen, setSaveViewOpen] = React.useState(false);
  const [saveViewSeed, setSaveViewSeed] = React.useState<
    Partial<FilterBuilderState> | undefined
  >(undefined);

  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  // Always call both hooks (rules of hooks), but the inactive one
  // sits idle via `enabled: false` so we never fire two fetches per
  // render. Switching modes flips which one is enabled and that hook
  // resumes from its cache.
  const seriesQuery = useSeriesListInfinite(seriesFilters, {
    enabled: mode === "series",
  });
  const issueQuery = useIssuesCrossListInfinite(issueFilters, {
    enabled: mode === "issues",
  });
  // Each branch produces homogeneous arrays so the conditional
  // rendering below can rely on the narrowed types directly.
  const seriesItems = seriesQuery.data?.pages.flatMap((p) => p.items) ?? [];
  const issueItems = issueQuery.data?.pages.flatMap((p) => p.items) ?? [];
  const query = mode === "series" ? seriesQuery : issueQuery;
  const items = mode === "series" ? seriesItems : issueItems;

  // Multi-select (audit B3): the grid is where users actually browse,
  // so "filter all 2019 one-shots → mark read" has to work here. One
  // `useSelection` over the active mode's items; switching modes swaps
  // `items` (different first id) which auto-clears the set, and we exit
  // select mode on the switch so the toolbar doesn't linger empty.
  // "Select all matching" is intentionally absent — it needs a
  // cross-list server bulk endpoint (audit B17, deferred); only the
  // loaded set is actionable here, same as the saved-view surfaces.
  const isSeriesMode = mode === "series";
  // `items` is a union of two homogeneous arrays (series-mode vs
  // issue-mode); both element types carry `id`, so pin the selection's
  // type param to the shared `{ id }` shape rather than the union.
  const selection = useSelection<{ id: string }>(items);
  const bulkMarkIssues = useBulkMarkProgress();
  const bulkMarkSeries = useBulkMarkSeriesProgress();
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const [markReadOpen, setMarkReadOpen] = React.useState(false);
  const selectButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = React.useRef(false);
  const isMarkPending = bulkMarkIssues.isPending || bulkMarkSeries.isPending;

  const handleMode = React.useCallback(
    (m: LibraryGridMode) => {
      selection.exit();
      setMode(m);
    },
    [selection, setMode],
  );

  // Actually write the progress. `submitMarkRead` runs after the
  // backfill prompt (mark-read at scale); mark-unread skips the prompt.
  const submitMarkRead = React.useCallback(
    (backfill: boolean) => {
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      const onSuccess = () => {
        selection.exit();
        setMarkReadOpen(false);
      };
      if (isSeriesMode) {
        bulkMarkSeries.mutate(
          { series_ids: ids, finished: true, backfill },
          { onSuccess },
        );
      } else {
        bulkMarkIssues.mutate(
          { issue_ids: ids, finished: true, backfill },
          { onSuccess },
        );
      }
    },
    [isSeriesMode, bulkMarkSeries, bulkMarkIssues, selection],
  );

  const runBulk = React.useCallback(
    (finished: boolean) => {
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      // Mark-read at scale (≥ threshold) is overwhelmingly catalog
      // maintenance — prompt before backfilling the reading log. In
      // series mode each selected series fans out to many issues
      // server-side, so the series count is gated at the same number.
      if (finished && ids.length >= BULK_BACKFILL_PROMPT_THRESHOLD) {
        setMarkReadOpen(true);
        return;
      }
      const onSuccess = () => selection.exit();
      if (isSeriesMode) {
        bulkMarkSeries.mutate({ series_ids: ids, finished }, { onSuccess });
      } else {
        bulkMarkIssues.mutate({ issue_ids: ids, finished }, { onSuccess });
      }
    },
    [isSeriesMode, bulkMarkSeries, bulkMarkIssues, selection],
  );

  const selectedTargets = Array.from(selection.selected).map((id) => ({
    entry_kind: isSeriesMode ? ("series" as const) : ("issue" as const),
    ref_id: id,
  }));

  // Esc exits select mode; Cmd/Ctrl+A selects every loaded card.
  // Dormant while focus is in a form field (search input, etc.).
  React.useEffect(() => {
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
  // Restore focus to the Select trigger after leaving select mode.
  React.useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);

  // Infinite fetch is driven from inside `VirtualizedCardGrid` (last
  // virtual row nearing the loaded end) — the windowed DOM makes a
  // bottom IntersectionObserver sentinel unreliable.
  const gridStyle: React.CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };

  // Server populates `total` only on the first page (no cursor) so
  // subsequent pages don't pay a `COUNT(*)`. We always read off
  // `pages[0]` — that's the only place it's set, and it carries the
  // accurate total for the current filter set. Falls back to the old
  // "loaded-so-far + plus sign" string while the first page is in
  // flight or on legacy servers that don't send the field.
  const total = query.data?.pages[0]?.total ?? null;
  const totalLabel: string =
    total != null
      ? String(total)
      : items.length + (query.hasNextPage ? "+" : "");
  const itemNoun = mode === "series" ? "series" : "issue";
  const itemNounPlural = mode === "series" ? "series" : "issues";
  const itemLabel = total === 1 ? itemNoun : itemNounPlural;

  function handleSaveView() {
    const today = new Date().toISOString().slice(0, 10);
    const result = libraryGridStateToFilterBuilderState(
      {
        status,
        metadataCompleteness,
        yearFrom,
        yearTo,
        publishers,
        languages,
        ageRatings,
        genres,
        tags,
        credits,
        characters,
        teams,
        locations,
        ratingRange,
        trimmedQ,
      },
      today,
    );
    for (const facet of result.droppedFacets) {
      toast.warning(`${facet} filter can't be saved to a view yet — skipped`);
    }
    setSaveViewSeed(result.state);
    setSaveViewOpen(true);
  }

  return (
    <>
      <div className="mb-6 flex flex-wrap items-baseline justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">
            {libraryName}
          </h1>
          <p className="text-muted-foreground mt-1 text-sm">
            {libraryId == null && libraryCount != null
              ? `${libraryCount} ${libraryCount === 1 ? "library" : "libraries"} · ${totalLabel} ${itemLabel}`
              : `${totalLabel} ${itemLabel}`}
          </p>
        </div>
      </div>

      <LibraryGridToolbar
        mode={mode}
        onMode={handleMode}
        q={q}
        onQ={setQ}
        trimmedQ={trimmedQ}
        seriesSort={seriesSort}
        onSeriesSort={setSeriesSort}
        issueSort={issueSort}
        onIssueSort={setIssueSort}
        order={order}
        onOrder={setOrder}
        facetCount={facetCount}
        onOpenFilters={() => setFilterOpen(true)}
        canSaveView={facetCount > 0}
        onSaveView={handleSaveView}
        onClearFacets={clearFacets}
        cardSize={cardSize}
        onCardSize={setCardSize}
        cardSizeMin={CARD_SIZE_MIN}
        cardSizeMax={CARD_SIZE_MAX}
        cardSizeStep={CARD_SIZE_STEP}
        cardSizeDefault={CARD_SIZE_DEFAULT}
        canSelect={items.length > 0}
        selectMode={selection.selectMode}
        onEnterSelect={() => selection.enter()}
        onExitSelect={() => selection.exit()}
        selectButtonRef={selectButtonRef}
      />

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
            disabled: isMarkPending || selection.count === 0,
          },
          {
            id: "mark-unread",
            label: "Mark unread",
            icon: Circle,
            onClick: () => runBulk(false),
            disabled: isMarkPending || selection.count === 0,
          },
        ]}
        overflow={[
          {
            id: "add-to-collection",
            label: "Add to collection…",
            icon: FolderPlus,
            onClick: () => setPickerOpen(true),
            disabled: selection.count === 0,
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={isMarkPending}
      />
      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) selection.exit();
        }}
        targets={selectedTargets}
      />
      <BulkMarkReadDialog
        open={markReadOpen}
        onOpenChange={setMarkReadOpen}
        count={selection.count}
        onConfirm={submitMarkRead}
        isPending={isMarkPending}
      />

      {facetCount > 0 ? (
        <ActiveChips
          status={status}
          metadataCompleteness={metadataCompleteness}
          readStatus={readStatus}
          yearFrom={yearFrom}
          yearTo={yearTo}
          ratingRange={ratingRange}
          publishers={publishers}
          languages={languages}
          ageRatings={ageRatings}
          genres={genres}
          tags={tags}
          credits={credits}
          anyCredits={anyCredits}
          characters={characters}
          teams={teams}
          locations={locations}
          onClearStatus={() => setStatus("any")}
          onClearMetadataCompleteness={() => setMetadataCompleteness(undefined)}
          onRemoveReadStatus={(v) =>
            setReadStatus(readStatus.filter((x) => x !== v))
          }
          onClearYear={() => {
            setYearFrom("");
            setYearTo("");
          }}
          onClearRating={() => setRatingRange(null)}
          onRemovePublisher={(v) =>
            setPublishers(publishers.filter((x) => x !== v))
          }
          onRemoveLanguage={(v) =>
            setLanguages(languages.filter((x) => x !== v))
          }
          onRemoveAgeRating={(v) =>
            setAgeRatings(ageRatings.filter((x) => x !== v))
          }
          onRemoveGenre={(v) => setGenres(genres.filter((x) => x !== v))}
          onRemoveTag={(v) => setTags(tags.filter((x) => x !== v))}
          onRemoveCredit={(role, v) =>
            setCreditRole(
              role,
              credits[role].filter((x) => x !== v),
            )
          }
          onRemoveAnyCredit={(v) =>
            setAnyCredits(anyCredits.filter((x) => x !== v))
          }
          onRemoveCharacter={(v) =>
            setCharacters(characters.filter((x) => x !== v))
          }
          onRemoveTeam={(v) => setTeams(teams.filter((x) => x !== v))}
          onRemoveLocation={(v) =>
            setLocations(locations.filter((x) => x !== v))
          }
        />
      ) : null}

      {/* A–Z jump rail (B9): series mode only — maps to the server
          `starts_with` filter on normalized_name. Issues sort by number,
          so the rail is meaningless there. */}
      {isSeriesMode ? (
        <AtoZJumpRail
          value={startsWith}
          onSelect={setStartsWith}
          className="mb-2"
        />
      ) : null}

      {/* Worklist churn (B4): when the grid is filtered to the
          needs-metadata worklist, offer to walk the loaded series through
          the match dialog with auto-advance after each apply. */}
      {isSeriesMode && metadataCompleteness === "needs_metadata" ? (
        <div className="mb-4">
          <MetadataWorklistButton
            series={seriesItems.map((s) => ({
              seriesSlug: s.slug,
              libraryId: s.library_id,
              name: s.name,
            }))}
          />
        </div>
      ) : null}

      {query.isError ? (
        <p className="text-destructive text-sm">
          Failed to load {mode === "series" ? "series" : "issues"}.{" "}
          {String(query.error)}
        </p>
      ) : null}

      {query.isLoading ? (
        <GridSkeleton mode={mode} style={gridStyle} />
      ) : items.length === 0 ? (
        <GridEmptyState
          mode={mode}
          facetCount={facetCount}
          hasQuery={!!trimmedQ}
          onClearFacets={clearFacets}
        />
      ) : (
        <VirtualizedCardGrid
          items={items}
          cardSize={cardSize}
          estimateTextHeight={isSeriesMode ? SERIES_TEXT_H : ISSUE_TEXT_H}
          hasNextPage={!!query.hasNextPage}
          isFetchingNextPage={query.isFetchingNextPage}
          fetchNextPage={() => void query.fetchNextPage()}
          enableScrollRestore
          renderCard={(item) =>
            isSeriesMode ? (
              <SeriesCard
                series={item as (typeof seriesItems)[number]}
                size="md"
                selectMode={
                  selection.selectMode
                    ? {
                        isActive: true,
                        isSelected: selection.isSelected(item.id),
                        onToggle: (ev) => selection.toggle(item.id, ev),
                      }
                    : undefined
                }
                onEnterSelectMode={(id) => selection.toggle(id)}
              />
            ) : (
              <IssueCard
                issue={item as (typeof issueItems)[number]}
                selectMode={
                  selection.selectMode
                    ? {
                        isActive: true,
                        isSelected: selection.isSelected(item.id),
                        onToggle: (ev) => selection.toggle(item.id, ev),
                      }
                    : undefined
                }
                onEnterSelectMode={(id) => selection.toggle(id)}
              />
            )
          }
        />
      )}

      <FilterSheet
        open={filterOpen}
        onOpenChange={setFilterOpen}
        mode={mode}
        libraryId={libraryId}
        status={status}
        onStatus={setStatus}
        metadataCompleteness={metadataCompleteness}
        onMetadataCompleteness={setMetadataCompleteness}
        readStatus={readStatus}
        onReadStatus={setReadStatus}
        yearFrom={yearFrom}
        yearTo={yearTo}
        onYearFrom={setYearFrom}
        onYearTo={setYearTo}
        ratingRange={ratingRange}
        onRatingRange={setRatingRange}
        publishers={publishers}
        onPublishers={setPublishers}
        languages={languages}
        onLanguages={setLanguages}
        ageRatings={ageRatings}
        onAgeRatings={setAgeRatings}
        genres={genres}
        onGenres={setGenres}
        tags={tags}
        onTags={setTags}
        credits={credits}
        onCredit={setCreditRole}
        characters={characters}
        onCharacters={setCharacters}
        teams={teams}
        onTeams={setTeams}
        locations={locations}
        onLocations={setLocations}
        activeCount={facetCount}
        onClear={clearFacets}
      />

      <NewFilterViewDialog
        open={saveViewOpen}
        onOpenChange={setSaveViewOpen}
        initial={saveViewSeed}
        autoPin
      />
    </>
  );
}

function GridSkeleton({
  mode,
  style,
}: {
  mode: LibraryGridMode;
  style: React.CSSProperties;
}) {
  return (
    <ul role="list" className="grid gap-4" style={style}>
      {Array.from({ length: 12 }).map((_, i) => (
        <li key={i}>
          {mode === "series" ? (
            <SeriesCardSkeleton size="md" />
          ) : (
            <IssueCardSkeleton />
          )}
        </li>
      ))}
    </ul>
  );
}

function GridEmptyState({
  mode,
  facetCount,
  hasQuery,
  onClearFacets,
}: {
  mode: LibraryGridMode;
  facetCount: number;
  hasQuery: boolean;
  onClearFacets: () => void;
}) {
  const noun = mode === "series" ? "series" : "issues";
  let message: string;
  if (hasQuery && facetCount > 0) {
    message = "No matches for the current search and filters.";
  } else if (hasQuery) {
    message = "No matches for that search.";
  } else if (facetCount > 0) {
    message = `No ${noun} match these filters.`;
  } else {
    message = `This library has no ${noun} yet.`;
  }
  return (
    <EmptyState
      size="sm"
      description={message}
      // One-click recovery instead of hunting active chips in the
      // toolbar/sheet (audit A12/A20).
      action={
        facetCount > 0 ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={onClearFacets}
          >
            Clear filters
          </Button>
        ) : undefined
      }
    />
  );
}
