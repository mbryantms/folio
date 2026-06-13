"use client";

import * as React from "react";
import { toast } from "sonner";

import { ActiveChips } from "@/components/library/ActiveChips";
import { FilterSheet } from "@/components/library/FilterSheet";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { LibraryGridToolbar } from "@/components/library/LibraryGridToolbar";
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
import {
  useIssuesCrossListInfinite,
  useSeriesListInfinite,
} from "@/lib/api/queries";
import { useLibraryGridFilters } from "@/lib/library/use-grid-filters";
import { cn } from "@/lib/utils";

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
    readStatus,
    setReadStatus,
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

  // Auto-fetch the next page when the sentinel scrolls into view —
  // mirrors `IssuesPanel` so the cadence feels familiar. Depend on
  // the three fields, not the whole result object: TanStack returns a
  // fresh object identity every render, so `[query]` tore down and
  // recreated the observer on every keystroke/state change.
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
        onMode={setMode}
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
      />

      {facetCount > 0 ? (
        <ActiveChips
          status={status}
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

      {query.isError ? (
        <p className="text-destructive text-sm">
          Failed to load {mode === "series" ? "series" : "issues"}.{" "}
          {String(query.error)}
        </p>
      ) : null}

      {query.isLoading ? (
        <GridSkeleton mode={mode} style={gridStyle} />
      ) : items.length === 0 ? (
        <EmptyState
          mode={mode}
          facetCount={facetCount}
          hasQuery={!!trimmedQ}
          onClearFacets={clearFacets}
        />
      ) : mode === "series" ? (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {seriesItems.map((s) => (
            <li key={s.id}>
              <SeriesCard series={s} size="md" />
            </li>
          ))}
        </ul>
      ) : (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {issueItems.map((i) => (
            <li key={i.id}>
              <IssueCard issue={i} />
            </li>
          ))}
        </ul>
      )}

      <div
        ref={sentinelRef}
        aria-hidden
        className={cn("h-12", query.hasNextPage ? "" : "hidden")}
      />
      {query.isFetchingNextPage ? (
        <p
          role="status"
          className="text-muted-foreground mt-2 text-center text-xs"
        >
          Loading more…
        </p>
      ) : null}

      <FilterSheet
        open={filterOpen}
        onOpenChange={setFilterOpen}
        mode={mode}
        libraryId={libraryId}
        status={status}
        onStatus={setStatus}
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

function EmptyState({
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
    <div className="border-border/60 text-muted-foreground space-y-3 rounded-lg border border-dashed p-8 text-center text-sm">
      <p>{message}</p>
      {/* One-click recovery instead of hunting active chips in the
          toolbar/sheet (audit A12/A20). */}
      {facetCount > 0 ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={onClearFacets}
        >
          Clear filters
        </Button>
      ) : null}
    </div>
  );
}
