"use client";

import { ArrowLeft, Search, User } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";

import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { IssueCard } from "@/components/library/IssueCard";
import { SeriesCard } from "@/components/library/SeriesCard";
import { useCardSize } from "@/components/library/use-card-size";
import type { IssueSearchHit, SeriesView } from "@/lib/api/types";
import {
  useGlobalSearch,
  type GlobalSearchPayloads,
} from "@/lib/search/use-search";
import {
  SEARCH_CATEGORIES,
  type SearchCategory,
  type SearchCategoryDef,
  type SearchHit,
} from "@/lib/search/types";

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
 *     trailing "View all" tile deep-links to the category-filtered
 *     grid view.
 *   - **Category-filtered (`category === 'series' | 'issues' | 'people'`)**:
 *     a single full-width grid of just that category, with a "Back to
 *     all results" link. Mirrors the destination of the rail's View all
 *     tile.
 *
 * Card-size slider drives the cover-card width on Series + Issues rails
 * / grids; people tiles use the same width so the rail stays visually
 * uniform.
 */
export function SearchView({
  initialQuery,
  category,
}: {
  initialQuery: string;
  category: SearchCategory | null;
}) {
  const [raw, setRaw] = useState(initialQuery);
  const [debounced, setDebounced] = useState(initialQuery.trim());
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
  // keystroke. `history.replaceState` updates the address bar in place;
  // a hard refresh still hydrates from `?q=` because the page reads
  // `searchParams` at request time. We preserve the `?category=` param
  // so deep-link to a category grid still updates `?q=` cleanly as the
  // user retypes.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const url = new URL(window.location.href);
    if (debounced.length > 0) url.searchParams.set("q", debounced);
    else url.searchParams.delete("q");
    window.history.replaceState({}, "", url.toString());
  }, [debounced]);

  // Omit `perCategory` so each backend serves its server-side max (the
  // old single `75` quietly clamped to 50 on the issues backend,
  // hiding rows from the rail). Modal usage still passes a small N.
  const { enabled, isLoading, groups, payloads, total } =
    useGlobalSearch(debounced);

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
        {activeDef ? null : (
          <p className="text-muted-foreground text-sm">
            Search across your library. Series, issues, and people are live.
          </p>
        )}
        <div className="border-border bg-card flex items-center gap-2 rounded-md border px-3 py-2 shadow-sm">
          <Search
            aria-hidden="true"
            className="text-muted-foreground size-4 shrink-0"
          />
          <input
            type="search"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            placeholder="Search series, issues, people…"
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
        />
      </header>

      {activeDef ? (
        <CategoryGrid
          def={activeDef}
          query={debounced}
          enabled={enabled}
          payloads={payloads}
          groups={groups}
          cardSize={cardSize}
        />
      ) : (
        <div className="space-y-8">
          {SEARCH_CATEGORIES.map((def) => (
            <CategoryRail
              key={def.key}
              def={def}
              hits={groups[def.key]}
              payloads={payloads}
              query={debounced}
              enabled={enabled}
              cardSize={cardSize}
            />
          ))}
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
}: {
  enabled: boolean;
  isLoading: boolean;
  total: number;
  query: string;
  activeDef: SearchCategoryDef | null;
  payloads: GlobalSearchPayloads;
  groups: { [K in SearchCategory]: SearchHit[] };
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
    ? categoryCount(activeDef.key, payloads, groups)
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
  const count = categoryCountForRail(def.key, payloads, hits);
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
    (def.key === "people" && hits.length === 0);
  if (empty) {
    return <NoMatches query={query} labelPlural={def.labelPlural} />;
  }
  const viewAllHref = `/search?q=${encodeURIComponent(query)}&category=${def.key}`;
  const itemStyle: React.CSSProperties = { width: `${cardSize}px` };
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
  itemStyle: React.CSSProperties,
): React.ReactNode {
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
      <PersonCard hit={hit} />
    </div>
  ));
}

function CategoryGrid({
  def,
  query,
  enabled,
  payloads,
  groups,
  cardSize,
}: {
  def: SearchCategoryDef;
  query: string;
  enabled: boolean;
  payloads: GlobalSearchPayloads;
  groups: { [K in SearchCategory]: SearchHit[] };
  cardSize: number;
}) {
  if (!enabled) {
    return (
      <p className="text-muted-foreground text-sm">
        Type at least 2 characters to see {def.labelPlural}.
      </p>
    );
  }
  const gridStyle: React.CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };
  if (def.key === "series") {
    if (payloads.series.length === 0) {
      return <NoMatches query={query} labelPlural={def.labelPlural} />;
    }
    return <SeriesGrid series={payloads.series} gridStyle={gridStyle} />;
  }
  if (def.key === "issues") {
    if (payloads.issues.length === 0) {
      return <NoMatches query={query} labelPlural={def.labelPlural} />;
    }
    return <IssuesGrid issues={payloads.issues} gridStyle={gridStyle} />;
  }
  if (groups.people.length === 0) {
    return <NoMatches query={query} labelPlural={def.labelPlural} />;
  }
  return <PeopleGrid hits={groups.people} gridStyle={gridStyle} />;
}

function SeriesGrid({
  series,
  gridStyle,
}: {
  series: ReadonlyArray<SeriesView>;
  gridStyle: React.CSSProperties;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {series.map((s) => (
        <li key={s.id}>
          <SeriesCard series={s} size="md" />
        </li>
      ))}
    </ul>
  );
}

function IssuesGrid({
  issues,
  gridStyle,
}: {
  issues: ReadonlyArray<IssueSearchHit>;
  gridStyle: React.CSSProperties;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {issues.map((i) => (
        <li key={i.id}>
          <IssueCard issue={i} />
        </li>
      ))}
    </ul>
  );
}

function PeopleGrid({
  hits,
  gridStyle,
}: {
  hits: ReadonlyArray<SearchHit>;
  gridStyle: React.CSSProperties;
}) {
  return (
    <ul role="list" className="grid gap-4" style={gridStyle}>
      {hits.map((hit) => (
        <li key={hit.id}>
          <PersonCard hit={hit} />
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

/** Cover-shaped tile for a person hit — same 2:3 footprint as
 *  `SeriesCard` / `IssueCard` so rails read uniformly. The icon stands
 *  in for the cover; the title + subtitle slot mirrors the cards
 *  below. */
function PersonCard({ hit }: { hit: SearchHit }) {
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
