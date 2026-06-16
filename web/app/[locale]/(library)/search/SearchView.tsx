"use client";

import {
  ArrowLeft,
  Check,
  ChevronRight,
  Circle,
  Filter,
  FolderPlus,
  Loader2,
  Search,
  User,
  X,
} from "lucide-react";
import Link from "next/link";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ComponentType,
  type CSSProperties,
  type ReactNode,
} from "react";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import {
  BulkMarkReadDialog,
  BULK_BACKFILL_PROMPT_THRESHOLD,
} from "@/components/library/BulkMarkReadDialog";
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { IssueCard } from "@/components/library/IssueCard";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { SeriesCard } from "@/components/library/SeriesCard";
import { useCardSize } from "@/components/library/use-card-size";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  useBulkMarkProgress,
  useBulkMarkSeriesProgress,
} from "@/lib/api/mutations";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";
import { useCoarsePointerActionsHint } from "@/lib/ui/use-coarse-pointer";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import type { IssueSummaryView, SeriesView } from "@/lib/api/types";
import {
  useIssuesCrossListInfinite,
  useSeriesListInfinite,
  type SeriesListFilters,
} from "@/lib/api/queries";
import { renderSearchSnippet } from "@/lib/search/render-snippet";
import {
  useGlobalSearch,
  type GlobalSearchPayloads,
  type GlobalSearchTotals,
} from "@/lib/search/use-search";
import {
  SEARCH_CATEGORIES,
  type SearchCategory,
  type SearchCategoryDef,
  type SearchHit,
} from "@/lib/search/types";
import {
  EMPTY_SERIES_SEARCH_FILTERS,
  SERIES_SEARCH_SORT_OPTIONS,
  SERIES_STATUS_OPTIONS,
  countActiveSeriesFilters,
  seriesSearchFiltersToHook,
  seriesSearchFiltersToParams,
  type SeriesSearchFilterState,
  type SeriesSearchSort,
} from "@/lib/search/series-search-filters";

const QUERY_DEBOUNCE_MS = 250;

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.search.cardSize";

/**
 * Full-page search experience. Two layouts share the same fetch + input
 * scaffolding:
 *
 *   - **Default (`category === null`)**: one horizontal-scroll rail per
 *     category, capped at a sensible preview window. Each rail's
 *     trailing "Top results" link deep-links to the category-filtered
 *     grid view.
 *   - **Category-filtered (`category === 'series' | 'issues' | 'people'`)**:
 *     a single full-width grid of just that category, with a "Back to
 *     all results" link. Mirrors the destination of the rail's top-results
 *     tile.
 *
 * Card-size slider drives the cover-card width on Series + Issues rails
 * / grids; people tiles use the same width so the rail stays visually
 * uniform.
 */
export function SearchView({
  initialQuery,
  category,
  initialFilters,
}: {
  initialQuery: string;
  category: SearchCategory | null;
  /** Server-parsed initial filter state (sort + facets) for the
   *  series category. Null when no filter params were on the URL. */
  initialFilters?: SeriesSearchFilterState;
}) {
  // One-time touch hint for the now-persistent cover kebab (audit B16).
  useCoarsePointerActionsHint();
  const [raw, setRaw] = useState(initialQuery);
  const [debounced, setDebounced] = useState(initialQuery.trim());
  const [filters, setFilters] = useState<SeriesSearchFilterState>(
    () => initialFilters ?? EMPTY_SERIES_SEARCH_FILTERS,
  );
  const [filterOpen, setFilterOpen] = useState(false);
  // Filters apply only on the series category grid. Anywhere else the
  // hook ignores them, so we just hide the Sort/Filter affordances.
  const filtersActive = category === "series";
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  useEffect(() => {
    const t = setTimeout(() => setDebounced(raw.trim()), QUERY_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [raw]);

  // Keep the URL in sync without forcing an RSC re-render on every
  // keystroke. `history.replaceState` updates the address bar in
  // place; a hard refresh still hydrates from the query string
  // because the page reads `searchParams` at request time. The
  // filter params join the same dance so deep-linking a sorted /
  // filtered grid works on reload.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    if (debounced.length > 0) url.searchParams.set("q", debounced);
    else url.searchParams.delete("q");
    // Filter params only mean something on the series category grid;
    // strip them otherwise to keep URLs clean across category nav.
    const filterParams = filtersActive
      ? seriesSearchFiltersToParams(filters)
      : {};
    for (const key of [
      "sort",
      "year_from",
      "year_to",
      "status",
      "publisher",
      "library",
    ] as const) {
      const value = filterParams[key];
      if (value) url.searchParams.set(key, value);
      else url.searchParams.delete(key);
    }
    window.history.replaceState({}, "", url.toString());
  }, [debounced, filters, filtersActive]);

  // Omit `perCategory` so each backend serves its server-side max (the
  // old single `75` quietly clamped to 50 on the issues backend,
  // hiding rows from the rail). Modal usage still passes a small N.
  const { enabled, isLoading, groups, payloads, categoryTotals, total } =
    useGlobalSearch(
      debounced,
      filtersActive
        ? { seriesFilters: seriesSearchFiltersToHook(filters) }
        : {},
    );

  const activeDef = category
    ? SEARCH_CATEGORIES.find((c) => c.key === category)
    : null;

  return (
    <div className="space-y-6">
      <header className="space-y-3">
        <div className="flex flex-wrap items-baseline justify-between gap-4">
          <div className="min-w-0">
            {activeDef ? (
              <Link
                href={`/search?q=${encodeURIComponent(debounced)}`}
                className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs"
              >
                <ArrowLeft className="size-3" />
                Back to all results
              </Link>
            ) : null}
            <h1 className="mt-0.5 text-2xl font-semibold tracking-tight capitalize">
              {activeDef ? activeDef.labelPlural : "Search"}
            </h1>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {filtersActive ? (
              <SeriesSortDropdown
                value={filters.sort}
                onChange={(sort) => setFilters((f) => ({ ...f, sort }))}
              />
            ) : null}
            {filtersActive ? (
              <SeriesFilterSheet
                open={filterOpen}
                onOpenChange={setFilterOpen}
                filters={filters}
                onChange={setFilters}
                activeCount={countActiveSeriesFilters(filters)}
              />
            ) : null}
            <CardSizeOptions
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
              description="Adjust card size for Series, Issues, and People rails."
            />
          </div>
        </div>
        {activeDef ? null : (
          <p className="text-muted-foreground text-sm">
            Search across your library. Series, issues, bookmarks, and people
            are live.
          </p>
        )}
        <div className="border-border bg-card focus-within:ring-ring flex items-center gap-2 rounded-md border px-3 py-2 shadow-sm focus-within:ring-2">
          <Search
            aria-hidden="true"
            className="text-muted-foreground size-4 shrink-0"
          />
          <input
            type="search"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            placeholder="Search series, issues, bookmarks, people…"
            aria-label="Search the library"
            autoFocus
            className="placeholder:text-muted-foreground w-full bg-transparent text-sm focus:outline-none"
          />
        </div>
        <SummaryLine
          enabled={enabled}
          isLoading={isLoading}
          total={total}
          query={debounced}
          activeDef={activeDef ?? null}
          payloads={payloads}
          groups={groups}
          categoryTotals={categoryTotals}
        />
      </header>

      {activeDef ? (
        <CategoryGrid
          def={activeDef}
          query={debounced}
          enabled={enabled}
          groups={groups}
          cardSize={cardSize}
          seriesFilters={
            filtersActive ? seriesSearchFiltersToHook(filters) : {}
          }
        />
      ) : (
        <div className="space-y-8">
          {SEARCH_CATEGORIES.map((def) => (
            <CategoryRail
              key={def.key}
              def={def}
              hits={groups[def.key]}
              payloads={payloads}
              categoryTotals={categoryTotals}
              query={debounced}
              enabled={enabled}
              cardSize={cardSize}
            />
          ))}
          {/* All-empty fallback. The per-rail null-returns above hide
           *  empty categories individually; if EVERY rail was empty
           *  this band tells the user something rather than leaving
           *  the page silent. */}
          {enabled &&
          !isLoading &&
          total === 0 &&
          payloads.series.length === 0 &&
          payloads.issues.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No matches for &ldquo;{debounced}&rdquo; in any category.
            </p>
          ) : null}
        </div>
      )}
    </div>
  );
}

function SummaryLine({
  enabled,
  isLoading,
  total,
  query,
  activeDef,
  payloads,
  groups,
  categoryTotals,
}: {
  enabled: boolean;
  isLoading: boolean;
  total: number;
  query: string;
  activeDef: SearchCategoryDef | null;
  payloads: GlobalSearchPayloads;
  groups: { [K in SearchCategory]: SearchHit[] };
  categoryTotals: GlobalSearchTotals;
}) {
  if (!enabled) {
    return (
      <p className="text-muted-foreground text-xs">
        Type at least 2 characters to search.
      </p>
    );
  }
  if (isLoading && total === 0) {
    return <p className="text-muted-foreground text-xs">Searching…</p>;
  }
  const count = activeDef
    ? categoryTotals[activeDef.key] ||
      categoryCount(activeDef.key, payloads, groups)
    : total;
  const noun = activeDef
    ? count === 1
      ? activeDef.label.toLowerCase()
      : activeDef.labelPlural
    : count === 1
      ? "result"
      : "results";
  return (
    <p className="text-muted-foreground text-xs">
      {count} {noun} for{" "}
      <span className="text-foreground font-medium">&ldquo;{query}&rdquo;</span>
    </p>
  );
}

function categoryCount(
  key: SearchCategory,
  payloads: GlobalSearchPayloads,
  groups: { [K in SearchCategory]: SearchHit[] },
): number {
  if (key === "series") return payloads.series.length;
  if (key === "issues") return payloads.issues.length;
  return groups[key].length;
}

function CategoryRail({
  def,
  hits,
  payloads,
  categoryTotals,
  query,
  enabled,
  cardSize,
}: {
  def: SearchCategoryDef;
  hits: ReadonlyArray<SearchHit>;
  payloads: GlobalSearchPayloads;
  categoryTotals: GlobalSearchTotals;
  query: string;
  enabled: boolean;
  cardSize: number;
}) {
  const count =
    categoryTotals[def.key] || categoryCountForRail(def.key, payloads, hits);
  // Hide empty rails once we have results to compare against. Before
  // the query runs (enabled=false) we keep the placeholders so the
  // page doesn't visually collapse on every keystroke — the body's
  // own empty-state row reads "Awaiting query…" in that mode.
  if (enabled && count === 0) return null;
  // Promote the top-results link out of the rail's trailing slot into
  // the section header when the rail couldn't fit every match — the
  // horizontal-arrow tile alone is easy to miss past the first 8
  // cards, especially on touch.
  const viewAllHref = `/search?q=${encodeURIComponent(query)}&category=${def.key}`;
  const showViewAllLink = enabled && count > 0;
  return (
    <section className="space-y-3" data-category={def.key}>
      <header className="flex items-center gap-2">
        <h2 className="text-base font-semibold tracking-tight capitalize">
          {def.labelPlural}
        </h2>
        {enabled ? (
          <span className="text-muted-foreground text-xs">
            {count} {count === 1 ? "match" : "matches"}
          </span>
        ) : null}
        {showViewAllLink ? (
          <Link
            href={viewAllHref}
            className="text-muted-foreground hover:text-foreground ml-auto inline-flex items-center gap-1 text-xs font-medium"
          >
            Top results
            <ChevronRight className="size-3" aria-hidden="true" />
          </Link>
        ) : null}
      </header>
      <CategoryRailBody
        def={def}
        hits={hits}
        payloads={payloads}
        query={query}
        enabled={enabled}
        cardSize={cardSize}
      />
    </section>
  );
}

function categoryCountForRail(
  key: SearchCategory,
  payloads: GlobalSearchPayloads,
  hits: ReadonlyArray<SearchHit>,
): number {
  if (key === "series") return payloads.series.length;
  if (key === "issues") return payloads.issues.length;
  return hits.length;
}

function CategoryRailBody({
  def,
  hits,
  payloads,
  query,
  enabled,
  cardSize,
}: {
  def: SearchCategoryDef;
  hits: ReadonlyArray<SearchHit>;
  payloads: GlobalSearchPayloads;
  query: string;
  enabled: boolean;
  cardSize: number;
}) {
  if (!enabled) {
    return (
      <p className="text-muted-foreground text-xs">
        Awaiting query — start typing to see {def.labelPlural}.
      </p>
    );
  }
  const empty =
    (def.key === "series" && payloads.series.length === 0) ||
    (def.key === "issues" && payloads.issues.length === 0) ||
    // Every non-cover category — markers, people, and the saved-content
    // categories (views / collections / pages) — is hits-backed.
    (def.key !== "series" && def.key !== "issues" && hits.length === 0);
  if (empty) {
    return <NoMatches query={query} labelPlural={def.labelPlural} />;
  }
  const viewAllHref = `/search?q=${encodeURIComponent(query)}&category=${def.key}`;
  const itemStyle: CSSProperties = { width: `${cardSize}px` };
  return (
    <HorizontalScrollRail viewAllHref={viewAllHref} itemWidthPx={cardSize}>
      {renderRailItems(def, payloads, hits, itemStyle)}
    </HorizontalScrollRail>
  );
}

function renderRailItems(
  def: SearchCategoryDef,
  payloads: GlobalSearchPayloads,
  hits: ReadonlyArray<SearchHit>,
  itemStyle: CSSProperties,
): ReactNode {
  if (def.key === "series") {
    return payloads.series.map((s) => (
      <div key={s.id} style={itemStyle} className="shrink-0">
        <SeriesCard series={s} size="md" />
      </div>
    ));
  }
  if (def.key === "issues") {
    return payloads.issues.map((i) => (
      <div key={i.id} style={itemStyle} className="shrink-0">
        <IssueCard issue={i} />
      </div>
    ));
  }
  return hits.map((hit) => (
    <div key={hit.id} style={itemStyle} className="shrink-0">
      {def.key === "markers" ? (
        <MarkerSearchCard hit={hit} />
      ) : (
        <IconHitCard hit={hit} />
      )}
    </div>
  ));
}

function CategoryGrid({
  def,
  query,
  enabled,
  groups,
  cardSize,
  seriesFilters,
}: {
  def: SearchCategoryDef;
  query: string;
  enabled: boolean;
  groups: { [K in SearchCategory]: SearchHit[] };
  cardSize: number;
  seriesFilters: Partial<SeriesListFilters>;
}) {
  if (!enabled) {
    return (
      <p className="text-muted-foreground text-sm">
        Type at least 2 characters to see {def.labelPlural}.
      </p>
    );
  }
  const gridStyle: CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };
  if (def.key === "series") {
    return (
      <SeriesCategoryGrid
        query={query}
        enabled={enabled}
        filters={seriesFilters}
        gridStyle={gridStyle}
      />
    );
  }
  if (def.key === "issues") {
    return (
      <IssuesCategoryGrid
        query={query}
        enabled={enabled}
        gridStyle={gridStyle}
      />
    );
  }
  if (def.key === "markers") {
    if (groups.markers.length === 0) {
      return <NoMatches query={query} labelPlural={def.labelPlural} />;
    }
    return <MarkerGrid hits={groups.markers} gridStyle={gridStyle} />;
  }
  // People + the saved-content categories (views / collections / pages)
  // all render as generic icon-hit tiles from their `groups[key]` slice.
  if (groups[def.key].length === 0) {
    return <NoMatches query={query} labelPlural={def.labelPlural} />;
  }
  return <IconHitGrid hits={groups[def.key]} gridStyle={gridStyle} />;
}

function SeriesCategoryGrid({
  query,
  enabled,
  filters,
  gridStyle,
}: {
  query: string;
  enabled: boolean;
  filters: Partial<SeriesListFilters>;
  gridStyle: CSSProperties;
}) {
  const results = useSeriesListInfinite(
    enabled ? { q: query, limit: 60, ...filters } : {},
    { enabled },
  );
  const series = results.data?.pages.flatMap((p) => p.items) ?? [];
  const total = results.data?.pages[0]?.total ?? series.length;
  if (results.isLoading) {
    return <GridLoading label="series" />;
  }
  if (results.isError) {
    return <GridError label="series" />;
  }
  if (series.length === 0) {
    return <NoMatches query={query} labelPlural="series" />;
  }
  return (
    <div className="space-y-4">
      <SelectableResultGrid
        kind="series"
        items={series}
        total={total}
        gridStyle={gridStyle}
      />
      <LoadMoreButton
        hasNextPage={!!results.hasNextPage}
        isFetching={results.isFetchingNextPage}
        onClick={() => void results.fetchNextPage()}
      />
    </div>
  );
}

function IssuesCategoryGrid({
  query,
  enabled,
  gridStyle,
}: {
  query: string;
  enabled: boolean;
  gridStyle: CSSProperties;
}) {
  const results = useIssuesCrossListInfinite(
    enabled ? { q: query, limit: 60 } : {},
    { enabled },
  );
  const issues = results.data?.pages.flatMap((p) => p.items) ?? [];
  const total = results.data?.pages[0]?.total ?? issues.length;
  if (results.isLoading) {
    return <GridLoading label="issues" />;
  }
  if (results.isError) {
    return <GridError label="issues" />;
  }
  if (issues.length === 0) {
    return <NoMatches query={query} labelPlural="issues" />;
  }
  return (
    <div className="space-y-4">
      <SelectableResultGrid
        kind="issue"
        items={issues}
        total={total}
        gridStyle={gridStyle}
      />
      <LoadMoreButton
        hasNextPage={!!results.hasNextPage}
        isFetching={results.isFetchingNextPage}
        onClick={() => void results.fetchNextPage()}
      />
    </div>
  );
}

function GridLoading({ label }: { label: string }) {
  return (
    <p className="text-muted-foreground flex items-center gap-2 text-sm">
      <Loader2 className="size-4 animate-spin" />
      Loading {label}…
    </p>
  );
}

function GridError({ label }: { label: string }) {
  return (
    <p className="text-destructive text-sm">
      Failed to load {label}. Try again.
    </p>
  );
}

function LoadMoreButton({
  hasNextPage,
  isFetching,
  onClick,
}: {
  hasNextPage: boolean;
  isFetching: boolean;
  onClick: () => void;
}) {
  if (!hasNextPage) return null;
  return (
    <div className="flex justify-center">
      <Button
        type="button"
        variant="outline"
        onClick={onClick}
        disabled={isFetching}
      >
        {isFetching ? (
          <>
            <Loader2 className="mr-1.5 size-4 animate-spin" />
            Loading…
          </>
        ) : (
          "Load more"
        )}
      </Button>
    </div>
  );
}

function MarkerGrid({
  hits,
  gridStyle,
}: {
  hits: ReadonlyArray<SearchHit>;
  gridStyle: CSSProperties;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {hits.map((hit) => (
        <li key={hit.id}>
          <MarkerSearchCard hit={hit} />
        </li>
      ))}
    </ul>
  );
}

/** The slice of `useSelection` the result grids need — kept structural
 *  (and matching the hook's `toggle` signature) so the hook return
 *  passes straight through. */
type CardSelection = {
  selectMode: boolean;
  isSelected: (id: string) => boolean;
  toggle: (id: string, ev?: { shiftKey?: boolean }) => void;
};

/** Multi-select wrapper shared by the series + issues search result
 *  grids (audit B3). Owns the selection state, the SelectionToolbar, and
 *  the bulk dialogs, and renders the right card grid for `kind`. Like the
 *  library grid it offers "Select all loaded" but NOT "select all
 *  matching" — that needs a cross-list server bulk endpoint (audit B17,
 *  deferred). */
function SelectableResultGrid({
  kind,
  items,
  total,
  gridStyle,
}: {
  kind: "series" | "issue";
  items: SeriesView[] | IssueSummaryView[];
  total: number;
  gridStyle: CSSProperties;
}) {
  const isSeries = kind === "series";
  const selection = useSelection<{ id: string }>(items);
  const bulkMarkSeries = useBulkMarkSeriesProgress();
  const bulkMarkIssues = useBulkMarkProgress();
  const [pickerOpen, setPickerOpen] = useState(false);
  const [markReadOpen, setMarkReadOpen] = useState(false);
  const selectButtonRef = useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = useRef(false);
  const isPending = isSeries
    ? bulkMarkSeries.isPending
    : bulkMarkIssues.isPending;

  const submitMarkRead = useCallback(
    (backfill: boolean) => {
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      const onSuccess = () => {
        selection.exit();
        setMarkReadOpen(false);
      };
      if (isSeries) {
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
    [isSeries, bulkMarkSeries, bulkMarkIssues, selection],
  );

  const runBulk = useCallback(
    (finished: boolean) => {
      const ids = Array.from(selection.selected);
      if (ids.length === 0) return;
      if (finished && ids.length >= BULK_BACKFILL_PROMPT_THRESHOLD) {
        setMarkReadOpen(true);
        return;
      }
      const onSuccess = () => selection.exit();
      if (isSeries) {
        bulkMarkSeries.mutate({ series_ids: ids, finished }, { onSuccess });
      } else {
        bulkMarkIssues.mutate({ issue_ids: ids, finished }, { onSuccess });
      }
    },
    [isSeries, bulkMarkSeries, bulkMarkIssues, selection],
  );

  const selectedTargets = Array.from(selection.selected).map((id) => ({
    entry_kind: isSeries ? ("series" as const) : ("issue" as const),
    ref_id: id,
  }));

  // Esc exits; Cmd/Ctrl+A selects all loaded. Dormant in form fields.
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
  useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);

  const label = isSeries ? "series" : "issues";

  return (
    <>
      <div className="flex items-center justify-between gap-2">
        <p className="text-muted-foreground text-xs">
          Showing {items.length} of {total} {label}
        </p>
        <SelectModeButton
          ref={selectButtonRef}
          active={selection.selectMode}
          onEnter={() => selection.enter()}
          onExit={() => selection.exit()}
        />
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
            disabled: isPending || selection.count === 0,
          },
          {
            id: "mark-unread",
            label: "Mark unread",
            icon: Circle,
            onClick: () => runBulk(false),
            disabled: isPending || selection.count === 0,
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
        isPending={isPending}
      />

      {isSeries ? (
        <SeriesGrid
          series={items as SeriesView[]}
          gridStyle={gridStyle}
          selection={selection}
        />
      ) : (
        <IssuesGrid
          issues={items as IssueSummaryView[]}
          gridStyle={gridStyle}
          selection={selection}
        />
      )}

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
        isPending={isPending}
      />
    </>
  );
}

function SeriesGrid({
  series,
  gridStyle,
  selection,
}: {
  series: ReadonlyArray<SeriesView>;
  gridStyle: CSSProperties;
  selection?: CardSelection;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {series.map((s) => (
        <li key={s.id}>
          <SeriesCard
            series={s}
            size="md"
            selectMode={
              selection?.selectMode
                ? {
                    isActive: true,
                    isSelected: selection.isSelected(s.id),
                    onToggle: (ev) => selection.toggle(s.id, ev),
                  }
                : undefined
            }
            onEnterSelectMode={
              selection ? (id) => selection.toggle(id) : undefined
            }
          />
        </li>
      ))}
    </ul>
  );
}

function IssuesGrid({
  issues,
  gridStyle,
  selection,
}: {
  issues: ReadonlyArray<IssueSummaryView>;
  gridStyle: CSSProperties;
  selection?: CardSelection;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {issues.map((i) => (
        <li key={i.id}>
          <IssueCard
            issue={i}
            selectMode={
              selection?.selectMode
                ? {
                    isActive: true,
                    isSelected: selection.isSelected(i.id),
                    onToggle: (ev) => selection.toggle(i.id, ev),
                  }
                : undefined
            }
            onEnterSelectMode={
              selection ? (id) => selection.toggle(id) : undefined
            }
          />
        </li>
      ))}
    </ul>
  );
}

function IconHitGrid({
  hits,
  gridStyle,
}: {
  hits: ReadonlyArray<SearchHit>;
  gridStyle: CSSProperties;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {hits.map((hit) => (
        <li key={hit.id}>
          <IconHitCard hit={hit} />
        </li>
      ))}
    </ul>
  );
}

function NoMatches({
  query,
  labelPlural,
}: {
  query: string;
  labelPlural: string;
}) {
  return (
    <p className="text-muted-foreground text-sm">
      No {labelPlural} match &ldquo;{query}&rdquo;.
    </p>
  );
}

/** Cover-shaped tile for an icon-backed hit (people + the saved-content
 *  categories — views / collections / pages) — same 2:3 footprint as
 *  `SeriesCard` / `IssueCard` so rails read uniformly. `hit.icon` stands
 *  in for the cover; the title + subtitle slot mirrors the cards
 *  below. */
function IconHitCard({ hit }: { hit: SearchHit }) {
  const Icon = hit.icon ?? User;
  return (
    <Link
      href={hit.href}
      className="group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none"
    >
      <div
        aria-hidden="true"
        className="border-border bg-muted text-muted-foreground relative grid aspect-[2/3] w-full place-items-center overflow-hidden rounded-md border"
      >
        <Icon className="size-12 opacity-60" />
      </div>
      <div className="min-w-0 px-1">
        <div className="truncate text-sm font-medium" title={hit.title}>
          {hit.title}
        </div>
        {hit.subtitle ? (
          <div
            className="text-muted-foreground truncate text-xs"
            title={hit.subtitle}
          >
            {hit.subtitle}
          </div>
        ) : (
          <div className="text-muted-foreground text-xs">&nbsp;</div>
        )}
      </div>
    </Link>
  );
}

function MarkerSearchCard({ hit }: { hit: SearchHit }) {
  const Icon = hit.icon ?? User;
  return (
    <Link
      href={hit.href}
      className="group hover:bg-accent/40 focus-visible:ring-ring flex flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none"
    >
      <MarkerSearchThumbnail hit={hit} icon={Icon} />
      <div className="min-w-0 px-1">
        <div className="truncate text-sm font-medium" title={hit.title}>
          {hit.title}
        </div>
        {hit.subtitle ? (
          <div
            className="text-muted-foreground truncate text-xs"
            title={hit.subtitle}
          >
            {hit.subtitle}
          </div>
        ) : (
          <div className="text-muted-foreground text-xs">&nbsp;</div>
        )}
        {hit.snippet ? (
          <p
            className="text-muted-foreground [&_mark]:bg-primary/20 [&_mark]:text-foreground line-clamp-2 text-xs"
            dangerouslySetInnerHTML={{
              __html: renderSearchSnippet(hit.snippet),
            }}
          />
        ) : null}
      </div>
    </Link>
  );
}

function MarkerSearchThumbnail({
  hit,
  icon: Icon,
}: {
  hit: SearchHit;
  icon: ComponentType<{ className?: string }>;
}) {
  if (!hit.thumbUrl) {
    return (
      <div
        aria-hidden="true"
        className="border-border bg-muted text-muted-foreground relative grid aspect-[2/3] w-full place-items-center overflow-hidden rounded-md border"
      >
        <Icon className="size-12 opacity-60" />
      </div>
    );
  }

  const region = hit.region;
  if (!region) {
    return (
      <div className="border-border bg-muted relative aspect-[2/3] w-full overflow-hidden rounded-md border">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src={hit.thumbUrl}
          alt=""
          loading="lazy"
          className="absolute inset-0 h-full w-full object-cover transition group-hover:brightness-110"
        />
      </div>
    );
  }

  const scaleW = Math.min(100, 100 / Math.max(region.w, 1));
  const scaleH = Math.min(100, 100 / Math.max(region.h, 1));

  return (
    <div className="border-border bg-muted relative aspect-[2/3] w-full overflow-hidden rounded-md border">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src={hit.thumbUrl}
        alt=""
        loading="lazy"
        className="max-w-none transition group-hover:brightness-110"
        style={{
          position: "absolute",
          width: `${scaleW * 100}%`,
          height: `${scaleH * 100}%`,
          left: `${-region.x * scaleW}%`,
          top: `${-region.y * scaleH}%`,
        }}
      />
    </div>
  );
}

/** Inline `<Select>` that surfaces the available series-search sort
 *  orders. "Best match" is the relevance default (no `sort=` param on
 *  the URL). The visible label is short so the trigger fits next to
 *  the card-size + filter buttons without wrapping. */
function SeriesSortDropdown({
  value,
  onChange,
}: {
  value: SeriesSearchSort;
  onChange: (next: SeriesSearchSort) => void;
}) {
  return (
    <Select
      value={value}
      onValueChange={(v) => onChange(v as SeriesSearchSort)}
    >
      <SelectTrigger
        // Toolbar-row convention: h-9 to align with the CardSize +
        // Filters sibling controls. See `docs/dev/search.md`.
        className="h-9 min-w-36"
        aria-label="Sort search results"
      >
        <SelectValue placeholder="Sort" />
      </SelectTrigger>
      <SelectContent>
        {SERIES_SEARCH_SORT_OPTIONS.map((o) => (
          <SelectItem key={o.value} value={o.value}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/** Series-search facet sheet. Lighter than the full library-grid
 *  filter sheet — surfaces year range + status + publisher CSV +
 *  library on a single column, with an "Apply" + "Clear" footer.
 *  Filter state lives on the parent so the URL-sync effect picks
 *  up every mutation. */
function SeriesFilterSheet({
  open,
  onOpenChange,
  filters,
  onChange,
  activeCount,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  filters: SeriesSearchFilterState;
  onChange: React.Dispatch<React.SetStateAction<SeriesSearchFilterState>>;
  activeCount: number;
}) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-9"
          aria-label={
            activeCount > 0 ? `Filters (${activeCount} active)` : "Filters"
          }
        >
          <Filter className="mr-1 size-3.5" aria-hidden="true" />
          Filters
          {activeCount > 0 ? (
            <span className="bg-foreground text-background ml-1 inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] font-medium tabular-nums">
              {activeCount}
            </span>
          ) : null}
        </Button>
      </SheetTrigger>
      <SheetContent side="right" className="w-full max-w-sm space-y-6 px-5">
        <SheetHeader className="space-y-1 px-0">
          <SheetTitle>Refine series results</SheetTitle>
          <SheetDescription>
            Narrow the matches by year, status, publisher, or library. Filters
            apply only on the series category grid.
          </SheetDescription>
        </SheetHeader>
        <div className="space-y-5">
          <fieldset className="space-y-2">
            <legend className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Year
            </legend>
            <div className="flex items-center gap-2">
              <Input
                type="number"
                inputMode="numeric"
                placeholder="From"
                value={filters.yearFrom}
                onChange={(e) =>
                  onChange((f) => ({ ...f, yearFrom: e.target.value }))
                }
                aria-label="Year from"
                className="h-9"
              />
              <span className="text-muted-foreground text-xs">to</span>
              <Input
                type="number"
                inputMode="numeric"
                placeholder="To"
                value={filters.yearTo}
                onChange={(e) =>
                  onChange((f) => ({ ...f, yearTo: e.target.value }))
                }
                aria-label="Year to"
                className="h-9"
              />
            </div>
          </fieldset>
          <fieldset className="space-y-2">
            <legend className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Status
            </legend>
            <Select
              value={filters.status}
              onValueChange={(v) => onChange((f) => ({ ...f, status: v }))}
            >
              <SelectTrigger className="h-9">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SERIES_STATUS_OPTIONS.map((o) => (
                  <SelectItem key={o.value} value={o.value}>
                    {o.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </fieldset>
          <fieldset className="space-y-2">
            <legend className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Publisher
            </legend>
            <Input
              type="text"
              placeholder="Comma-separated (e.g. Marvel, DC)"
              value={filters.publishers.join(", ")}
              onChange={(e) =>
                onChange((f) => ({
                  ...f,
                  publishers: e.target.value
                    .split(",")
                    .map((s) => s.trim())
                    .filter(Boolean),
                }))
              }
              aria-label="Publishers"
              className="h-9"
            />
          </fieldset>
        </div>
        <div className="flex items-center justify-between gap-2 pt-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={activeCount === 0}
            onClick={() =>
              onChange((f) => ({
                ...EMPTY_SERIES_SEARCH_FILTERS,
                // Keep the sort selection independent of filters —
                // clearing facets shouldn't snap sort back to relevance.
                sort: f.sort,
              }))
            }
          >
            <X className="mr-1 size-3" aria-hidden="true" />
            Clear filters
          </Button>
          <Button type="button" size="sm" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  );
}
