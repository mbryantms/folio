"use client";

import * as React from "react";
import { ChevronDown, Filter, X } from "lucide-react";

import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import {
  CREDIT_ROLES,
  EMPTY_CREDITS,
  RATING_MAX,
  RATING_MIN,
  RATING_STEP,
} from "@/components/library/library-grid-filters";
import type {
  CreditKey,
  CreditState,
  LibraryGridInitialFilters,
  LibraryGridMode,
} from "@/components/library/library-grid-filters";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { useCardSize } from "@/components/library/use-card-size";
import { MultiSelectEditor } from "@/components/filters/value-editors/MultiSelectEditor";
import type { OptionsEndpoint } from "@/components/filters/field-registry";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PopoverPortalContainer } from "@/components/ui/popover";
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
} from "@/components/ui/sheet";
import { Slider } from "@/components/ui/slider";
import {
  useIssuesCrossListInfinite,
  useSeriesListInfinite,
} from "@/lib/api/queries";
import type {
  IssuesCrossListFilters,
  SeriesListFilters,
} from "@/lib/api/queries";
import type { IssueSort, SeriesSort, SortOrder } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.libraryGrid.cardSize";

/** Sort options surfaced in the dropdown for each mode. The `Recently
 *  …` labels match the verb users already see across the app; the
 *  release-date / rating / time-to-read additions sit at the bottom
 *  so the existing-default ordering keeps muscle memory. */
const SERIES_SORT_LABELS: Record<SeriesSort, string> = {
  name: "Name",
  created_at: "Recently added",
  updated_at: "Recently updated",
  year: "Release date",
};

const ISSUE_SORT_LABELS: Record<IssueSort, string> = {
  number: "Issue number",
  created_at: "Recently added",
  updated_at: "Recently updated",
  year: "Release date",
  page_count: "Time to read",
  user_rating: "My rating",
};

const STATUS_OPTIONS: { value: string; label: string }[] = [
  { value: "any", label: "Any status" },
  { value: "continuing", label: "Continuing" },
  { value: "ended", label: "Ended" },
  { value: "cancelled", label: "Cancelled" },
  { value: "hiatus", label: "Hiatus" },
];

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
  const init = initialFilters ?? {};
  const [mode, setMode] = React.useState<LibraryGridMode>(
    init.mode ?? "series",
  );
  const [q, setQ] = React.useState("");
  const [debouncedQ, setDebouncedQ] = React.useState("");
  // Sort state is mode-scoped: switching modes should not carry an
  // invalid sort across (e.g. `user_rating` is issue-only). We store
  // both as one union and validate before passing to the query.
  const [seriesSort, setSeriesSort] = React.useState<SeriesSort>("name");
  const [issueSort, setIssueSort] = React.useState<IssueSort>("number");
  const [order, setOrder] = React.useState<SortOrder>("asc");
  const [status, setStatus] = React.useState<string>(init.status ?? "any");
  const [yearFrom, setYearFrom] = React.useState<string>(init.yearFrom ?? "");
  const [yearTo, setYearTo] = React.useState<string>(init.yearTo ?? "");
  const [publishers, setPublishers] = React.useState<string[]>(
    init.publishers ?? [],
  );
  const [languages, setLanguages] = React.useState<string[]>(
    init.languages ?? [],
  );
  const [ageRatings, setAgeRatings] = React.useState<string[]>(
    init.ageRatings ?? [],
  );
  const [genres, setGenres] = React.useState<string[]>(init.genres ?? []);
  const [tags, setTags] = React.useState<string[]>(init.tags ?? []);
  const [credits, setCredits] = React.useState<CreditState>(() => ({
    ...EMPTY_CREDITS,
    ...(init.credits ?? {}),
  }));
  const [characters, setCharacters] = React.useState<string[]>(
    init.characters ?? [],
  );
  const [teams, setTeams] = React.useState<string[]>(init.teams ?? []);
  const [locations, setLocations] = React.useState<string[]>(
    init.locations ?? [],
  );
  const [ratingRange, setRatingRange] = React.useState<[number, number] | null>(
    init.ratingRange ?? null,
  );
  const [filterOpen, setFilterOpen] = React.useState(false);

  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  React.useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(q.trim()), 200);
    return () => clearTimeout(t);
  }, [q]);

  const trimmedQ = debouncedQ;

  // Filters shared by both modes — assembled once, then split per
  // endpoint below. `status` is series-only (issues don't carry
  // status) so it's left out of the issues filter shape.
  const sharedFilters = {
    library: libraryId ?? undefined,
    q: trimmedQ || undefined,
    order: trimmedQ ? undefined : order,
    year_from: parseYear(yearFrom),
    year_to: parseYear(yearTo),
    publisher: csvOrUndef(publishers),
    language: csvOrUndef(languages),
    age_rating: csvOrUndef(ageRatings),
    genres: csvOrUndef(genres),
    tags: csvOrUndef(tags),
    writers: csvOrUndef(credits.writers),
    pencillers: csvOrUndef(credits.pencillers),
    inkers: csvOrUndef(credits.inkers),
    colorists: csvOrUndef(credits.colorists),
    letterers: csvOrUndef(credits.letterers),
    cover_artists: csvOrUndef(credits.cover_artists),
    editors: csvOrUndef(credits.editors),
    translators: csvOrUndef(credits.translators),
    characters: csvOrUndef(characters),
    teams: csvOrUndef(teams),
    locations: csvOrUndef(locations),
    user_rating_min: ratingRange?.[0],
    user_rating_max: ratingRange?.[1],
    limit: 60,
  };

  const seriesFilters: SeriesListFilters = {
    ...sharedFilters,
    sort: trimmedQ ? undefined : seriesSort,
    status: status === "any" ? undefined : status,
  };
  const issueFilters: IssuesCrossListFilters = {
    ...sharedFilters,
    sort: trimmedQ ? undefined : issueSort,
  };

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
  // mirrors `IssuesPanel` so the cadence feels familiar.
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
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

  const creditCount = CREDIT_ROLES.reduce(
    (sum, c) => sum + credits[c.key].length,
    0,
  );
  const facetCount =
    (status !== "any" ? 1 : 0) +
    (yearFrom || yearTo ? 1 : 0) +
    (ratingRange ? 1 : 0) +
    publishers.length +
    languages.length +
    ageRatings.length +
    genres.length +
    tags.length +
    creditCount +
    characters.length +
    teams.length +
    locations.length;

  function clearFacets() {
    setStatus("any");
    setYearFrom("");
    setYearTo("");
    setRatingRange(null);
    setPublishers([]);
    setLanguages([]);
    setAgeRatings([]);
    setGenres([]);
    setTags([]);
    setCredits(EMPTY_CREDITS);
    setCharacters([]);
    setTeams([]);
    setLocations([]);
  }

  function setCreditRole(key: CreditKey, values: string[]) {
    setCredits((prev) => ({ ...prev, [key]: values }));
  }

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

      <div className="mb-6 flex flex-wrap items-center gap-2">
        {/* Mode toggle: two side-by-side buttons that match the rest
            of the toolbar (same `size="sm"` + outline base; active mode
            takes `variant="secondary"` for visual contrast). Earlier
            iterations used a bordered wrapper around the pair, which
            ran ~2px taller than the peer Sort / Order / Filters
            buttons — this version sits flush. */}
        <Button
          type="button"
          variant={mode === "series" ? "secondary" : "outline"}
          size="sm"
          aria-pressed={mode === "series"}
          onClick={() => setMode("series")}
        >
          Series
        </Button>
        <Button
          type="button"
          variant={mode === "issues" ? "secondary" : "outline"}
          size="sm"
          aria-pressed={mode === "issues"}
          onClick={() => setMode("issues")}
        >
          Issues
        </Button>
        <Input
          type="search"
          placeholder={mode === "series" ? "Search series…" : "Search issues…"}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          className="w-72"
        />
        {mode === "series" ? (
          <Select
            value={seriesSort}
            onValueChange={(v) => setSeriesSort(v as SeriesSort)}
          >
            <SelectTrigger className="w-44" disabled={!!trimmedQ}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(SERIES_SORT_LABELS) as SeriesSort[]).map((s) => (
                <SelectItem key={s} value={s}>
                  {SERIES_SORT_LABELS[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <Select
            value={issueSort}
            onValueChange={(v) => setIssueSort(v as IssueSort)}
          >
            <SelectTrigger className="w-44" disabled={!!trimmedQ}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(ISSUE_SORT_LABELS) as IssueSort[]).map((s) => (
                <SelectItem key={s} value={s}>
                  {ISSUE_SORT_LABELS[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!!trimmedQ}
          onClick={() => setOrder((o) => (o === "asc" ? "desc" : "asc"))}
          title={`Order: ${order === "asc" ? "Ascending" : "Descending"}`}
        >
          {order === "asc" ? "↑" : "↓"}
        </Button>

        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setFilterOpen(true)}
        >
          <Filter className="mr-1 h-3.5 w-3.5" />
          Filters
          {facetCount > 0 ? (
            <Badge
              variant="secondary"
              className="ml-2 h-5 min-w-5 rounded-full px-1.5 text-xs"
            >
              {facetCount}
            </Badge>
          ) : null}
        </Button>

        {facetCount > 0 ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={clearFacets}
            className="text-muted-foreground"
          >
            <X className="mr-1 h-3 w-3" /> Clear filters
          </Button>
        ) : null}

        <div className="ml-auto">
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
        </div>
      </div>

      {facetCount > 0 ? (
        <ActiveChips
          status={status}
          yearFrom={yearFrom}
          yearTo={yearTo}
          ratingRange={ratingRange}
          publishers={publishers}
          languages={languages}
          ageRatings={ageRatings}
          genres={genres}
          tags={tags}
          credits={credits}
          characters={characters}
          teams={teams}
          locations={locations}
          onClearStatus={() => setStatus("any")}
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
        <EmptyState mode={mode} facetCount={facetCount} hasQuery={!!trimmedQ} />
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
        <p className="text-muted-foreground mt-2 text-center text-xs">
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
    </>
  );
}

function FilterSheet({
  open,
  onOpenChange,
  mode,
  libraryId,
  status,
  onStatus,
  yearFrom,
  yearTo,
  onYearFrom,
  onYearTo,
  ratingRange,
  onRatingRange,
  publishers,
  onPublishers,
  languages,
  onLanguages,
  ageRatings,
  onAgeRatings,
  genres,
  onGenres,
  tags,
  onTags,
  credits,
  onCredit,
  characters,
  onCharacters,
  teams,
  onTeams,
  locations,
  onLocations,
  activeCount,
  onClear,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mode: LibraryGridMode;
  libraryId: string | null;
  status: string;
  onStatus: (v: string) => void;
  yearFrom: string;
  yearTo: string;
  onYearFrom: (v: string) => void;
  onYearTo: (v: string) => void;
  ratingRange: [number, number] | null;
  onRatingRange: (v: [number, number] | null) => void;
  publishers: string[];
  onPublishers: (v: string[]) => void;
  languages: string[];
  onLanguages: (v: string[]) => void;
  ageRatings: string[];
  onAgeRatings: (v: string[]) => void;
  genres: string[];
  onGenres: (v: string[]) => void;
  tags: string[];
  onTags: (v: string[]) => void;
  credits: CreditState;
  onCredit: (key: CreditKey, values: string[]) => void;
  characters: string[];
  onCharacters: (v: string[]) => void;
  teams: string[];
  onTeams: (v: string[]) => void;
  locations: string[];
  onLocations: (v: string[]) => void;
  activeCount: number;
  onClear: () => void;
}) {
  // Forward `library` to the options endpoints so per-library views
  // only surface values that exist in that library.
  const optsLibrary = libraryId ?? undefined;
  const ratingDraft: [number, number] = ratingRange ?? [RATING_MIN, RATING_MAX];
  // Re-anchor the descendant `MultiSelectEditor` popovers into the
  // SheetContent subtree. Without this they portal to document.body
  // and Radix's Sheet modal aria-hides them — items render but reject
  // focus/clicks. `overflow-visible` so a wide picker can extend past
  // the sheet edge when needed; the inner body div owns the scroll.
  const [portalContainer, setPortalContainer] =
    React.useState<HTMLElement | null>(null);
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        ref={setPortalContainer}
        side="right"
        className="flex w-full flex-col gap-0 overflow-visible p-0 sm:max-w-md"
      >
        <SheetHeader className="border-border/60 flex-row items-center justify-between border-b px-6 py-4 pr-12">
          <div>
            <SheetTitle>Filters</SheetTitle>
            <SheetDescription>
              {activeCount > 0
                ? `${activeCount} active`
                : "Narrow the library by metadata."}
            </SheetDescription>
          </div>
          {activeCount > 0 ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={onClear}
              className="h-8"
            >
              Clear all
            </Button>
          ) : null}
        </SheetHeader>
        <PopoverPortalContainer value={portalContainer}>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {/* Status is series-only (issues don't carry one) — hide
                the section when the grid is in issues mode rather
                than disabling, so the picker stays uncluttered. */}
            {mode === "series" ? (
              <Section title="Status" defaultOpen>
                <Select value={status} onValueChange={onStatus}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {STATUS_OPTIONS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Section>
            ) : null}
            <Section title="Year">
              <div className="flex items-center gap-2">
                <Input
                  type="number"
                  inputMode="numeric"
                  placeholder="From"
                  value={yearFrom}
                  onChange={(e) => onYearFrom(e.target.value)}
                />
                <span className="text-muted-foreground text-xs">—</span>
                <Input
                  type="number"
                  inputMode="numeric"
                  placeholder="To"
                  value={yearTo}
                  onChange={(e) => onYearTo(e.target.value)}
                />
              </div>
            </Section>
            <Section title="My rating">
              <div className="space-y-3">
                <div className="text-muted-foreground flex justify-between text-xs tabular-nums">
                  <span>{ratingDraft[0].toFixed(1)} ★</span>
                  <span>{ratingDraft[1].toFixed(1)} ★</span>
                </div>
                <Slider
                  min={RATING_MIN}
                  max={RATING_MAX}
                  step={RATING_STEP}
                  value={ratingDraft}
                  onValueChange={(v) => {
                    if (
                      v.length === 2 &&
                      v[0] !== undefined &&
                      v[1] !== undefined
                    ) {
                      onRatingRange([v[0], v[1]]);
                    }
                  }}
                />
                <p className="text-muted-foreground text-xs">
                  Series you haven&apos;t rated are excluded when this filter is
                  active.
                </p>
                {ratingRange ? (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => onRatingRange(null)}
                    className="h-7 px-2 text-xs"
                  >
                    Clear rating filter
                  </Button>
                ) : null}
              </div>
            </Section>
            <FacetMultiSection
              title="Publisher"
              value={publishers}
              onChange={onPublishers}
              endpoint={{ kind: "publishers" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Language"
              value={languages}
              onChange={onLanguages}
              endpoint={{ kind: "languages" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Age rating"
              value={ageRatings}
              onChange={onAgeRatings}
              endpoint={{ kind: "age_ratings" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Genres"
              value={genres}
              onChange={onGenres}
              endpoint={{ kind: "genres" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Tags"
              value={tags}
              onChange={onTags}
              endpoint={{ kind: "tags" }}
              library={optsLibrary}
            />
            <Section title="Credits">
              <div className="space-y-3">
                {CREDIT_ROLES.map((c) => (
                  <div key={c.key} className="space-y-1">
                    <Label className="text-xs font-medium">{c.label}</Label>
                    <MultiSelectEditor
                      value={credits[c.key]}
                      onChange={(v) => onCredit(c.key, v)}
                      endpoint={{ kind: "credits", role: c.role }}
                      library={optsLibrary}
                      placeholder={`Any ${c.label.toLowerCase()}`}
                    />
                  </div>
                ))}
              </div>
            </Section>
            <FacetMultiSection
              title="Characters"
              value={characters}
              onChange={onCharacters}
              endpoint={{ kind: "characters" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Teams"
              value={teams}
              onChange={onTeams}
              endpoint={{ kind: "teams" }}
              library={optsLibrary}
            />
            <FacetMultiSection
              title="Locations"
              value={locations}
              onChange={onLocations}
              endpoint={{ kind: "locations" }}
              library={optsLibrary}
            />
          </div>
        </PopoverPortalContainer>
      </SheetContent>
    </Sheet>
  );
}

function FacetMultiSection({
  title,
  value,
  onChange,
  endpoint,
  library,
}: {
  title: string;
  value: string[];
  onChange: (v: string[]) => void;
  endpoint: OptionsEndpoint;
  library?: string;
}) {
  return (
    <Section title={title} badge={value.length > 0 ? value.length : undefined}>
      <MultiSelectEditor
        value={value}
        onChange={onChange}
        endpoint={endpoint}
        library={library}
        placeholder={`Any ${title.toLowerCase()}`}
      />
    </Section>
  );
}

/** Native-`<details>` collapsible section. We avoid Radix Accordion
 *  because it isn't installed and the markup here is tiny — the
 *  built-in disclosure widget gives us animation-free expand/collapse
 *  without adding a dep. The summary is styled to match the screenshot
 *  in the design brief: uppercase label, chevron on the right. */
function Section({
  title,
  badge,
  defaultOpen,
  children,
}: {
  title: string;
  badge?: number;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  return (
    <details
      open={defaultOpen}
      className="group border-border/60 border-b last:border-b-0"
    >
      <summary className="hover:bg-accent/40 flex cursor-pointer list-none items-center justify-between px-6 py-3 text-xs font-semibold tracking-wider uppercase select-none [&::-webkit-details-marker]:hidden">
        <span className="flex items-center gap-2">
          {title}
          {badge && badge > 0 ? (
            <Badge
              variant="secondary"
              className="h-5 min-w-5 rounded-full px-1.5 text-[10px]"
            >
              {badge}
            </Badge>
          ) : null}
        </span>
        <ChevronDown className="text-muted-foreground h-4 w-4 transition-transform group-open:rotate-180" />
      </summary>
      <div className="space-y-2 px-6 pb-4">{children}</div>
    </details>
  );
}

function ActiveChips({
  status,
  yearFrom,
  yearTo,
  ratingRange,
  publishers,
  languages,
  ageRatings,
  genres,
  tags,
  credits,
  characters,
  teams,
  locations,
  onClearStatus,
  onClearYear,
  onClearRating,
  onRemovePublisher,
  onRemoveLanguage,
  onRemoveAgeRating,
  onRemoveGenre,
  onRemoveTag,
  onRemoveCredit,
  onRemoveCharacter,
  onRemoveTeam,
  onRemoveLocation,
}: {
  status: string;
  yearFrom: string;
  yearTo: string;
  ratingRange: [number, number] | null;
  publishers: string[];
  languages: string[];
  ageRatings: string[];
  genres: string[];
  tags: string[];
  credits: CreditState;
  characters: string[];
  teams: string[];
  locations: string[];
  onClearStatus: () => void;
  onClearYear: () => void;
  onClearRating: () => void;
  onRemovePublisher: (v: string) => void;
  onRemoveLanguage: (v: string) => void;
  onRemoveAgeRating: (v: string) => void;
  onRemoveGenre: (v: string) => void;
  onRemoveTag: (v: string) => void;
  onRemoveCredit: (role: CreditKey, v: string) => void;
  onRemoveCharacter: (v: string) => void;
  onRemoveTeam: (v: string) => void;
  onRemoveLocation: (v: string) => void;
}) {
  return (
    <div className="mb-4 flex flex-wrap gap-1.5">
      {status !== "any" ? (
        <Chip
          label={`Status: ${labelFor(STATUS_OPTIONS, status)}`}
          onRemove={onClearStatus}
        />
      ) : null}
      {yearFrom || yearTo ? (
        <Chip
          label={`Year: ${yearFrom || "…"}–${yearTo || "…"}`}
          onRemove={onClearYear}
        />
      ) : null}
      {ratingRange ? (
        <Chip
          label={`Rating: ${ratingRange[0].toFixed(1)}–${ratingRange[1].toFixed(1)} ★`}
          onRemove={onClearRating}
        />
      ) : null}
      {publishers.map((v) => (
        <Chip
          key={`pub-${v}`}
          label={`Publisher: ${v}`}
          onRemove={() => onRemovePublisher(v)}
        />
      ))}
      {languages.map((v) => (
        <Chip
          key={`lang-${v}`}
          label={`Language: ${v}`}
          onRemove={() => onRemoveLanguage(v)}
        />
      ))}
      {ageRatings.map((v) => (
        <Chip
          key={`age-${v}`}
          label={`Age: ${v}`}
          onRemove={() => onRemoveAgeRating(v)}
        />
      ))}
      {genres.map((v) => (
        <Chip
          key={`gen-${v}`}
          label={`Genre: ${v}`}
          onRemove={() => onRemoveGenre(v)}
        />
      ))}
      {tags.map((v) => (
        <Chip
          key={`tag-${v}`}
          label={`Tag: ${v}`}
          onRemove={() => onRemoveTag(v)}
        />
      ))}
      {CREDIT_ROLES.flatMap((c) =>
        credits[c.key].map((v) => (
          <Chip
            key={`${c.key}-${v}`}
            label={`${c.label.replace(/s$/, "")}: ${v}`}
            onRemove={() => onRemoveCredit(c.key, v)}
          />
        )),
      )}
      {characters.map((v) => (
        <Chip
          key={`char-${v}`}
          label={`Character: ${v}`}
          onRemove={() => onRemoveCharacter(v)}
        />
      ))}
      {teams.map((v) => (
        <Chip
          key={`team-${v}`}
          label={`Team: ${v}`}
          onRemove={() => onRemoveTeam(v)}
        />
      ))}
      {locations.map((v) => (
        <Chip
          key={`loc-${v}`}
          label={`Location: ${v}`}
          onRemove={() => onRemoveLocation(v)}
        />
      ))}
    </div>
  );
}

function Chip({ label, onRemove }: { label: string; onRemove: () => void }) {
  return (
    <Badge variant="secondary" className="gap-1 pr-1">
      {label}
      <button
        type="button"
        onClick={onRemove}
        className="hover:bg-muted-foreground/20 rounded-sm"
        aria-label={`Remove ${label}`}
      >
        <X className="h-3 w-3" />
      </button>
    </Badge>
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
}: {
  mode: LibraryGridMode;
  facetCount: number;
  hasQuery: boolean;
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
    <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
      {message}
    </div>
  );
}

function csvOrUndef(values: string[]): string | undefined {
  return values.length ? values.join(",") : undefined;
}

function parseYear(raw: string): number | undefined {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;
  const n = Number.parseInt(trimmed, 10);
  return Number.isFinite(n) ? n : undefined;
}

function labelFor(
  options: { value: string; label: string }[],
  value: string,
): string {
  return options.find((o) => o.value === value)?.label ?? value;
}
