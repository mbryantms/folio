"use client";

import {
  Bookmark,
  Folder,
  Highlighter,
  LayoutDashboard,
  ListChecks,
  Star,
  StickyNote,
  User,
  type LucideIcon,
} from "lucide-react";
import { useMemo } from "react";

import {
  useCollections,
  useIssueSearch,
  useMarkerSearch,
  useMePages,
  usePeopleSearch,
  useSavedViews,
  useSeriesList,
  type SeriesListFilters,
} from "@/lib/api/queries";
import type {
  IssueSearchHit,
  MarkerKind,
  MarkerSearchHit,
  PageView,
  SavedViewView,
  SeriesView,
} from "@/lib/api/types";
import { issueUrl, pageBytesUrl, seriesUrl, readerUrl } from "@/lib/urls";

import {
  EMPTY_SEARCH_GROUPS,
  type SearchCategory,
  type SearchGroups,
  type SearchHit,
} from "./types";

const MIN_QUERY_LEN = 2;

interface GlobalSearchOpts {
  /** Soft per-category cap. The modal passes a small number (~5) so the
   *  dropdown doesn't grow unbounded. When omitted, each backend is
   *  asked for its server-side max — so the search-page rails stop
   *  silently clamping (e.g., the issues backend caps at 50 regardless
   *  of `limit`, so the old shared `75` lied about its real ceiling). */
  perCategory?: number;
  /** Additional series-side filter params forwarded to `useSeriesList`
   *  — sort, year-range, status, publisher, library, etc. Used by the
   *  dedicated `/search?category=series` grid (M4 facets + sort);
   *  ignored by every other surface. The hook merges these on top of
   *  `q` + `limit` so callers don't need to construct the full shape. */
  seriesFilters?: Partial<SeriesListFilters>;
}

/** Server-side max per backend. Mirrors the `MAX_LIMIT` constants in
 *  `crates/server/src/api/{issues,people,series}.rs`. Used by the
 *  search-page rails when no explicit `perCategory` is supplied so the
 *  request never asks for more than the backend will return. */
const BACKEND_MAX = {
  series: 100,
  issues: 50,
  markers: 50,
  people: 100,
} as const;

/** Soft cap for the client-filtered saved-content categories (views,
 *  collections, pages). Unlike the backend categories these have no
 *  server `MAX_LIMIT` — they filter cached, per-user-bounded lists in
 *  memory — so the rails just cap at a sane ceiling when no explicit
 *  `perCategory` is supplied. */
const SAVED_CONTENT_MAX = 25;

const WANT_TO_READ_KEY = "want_to_read";

/** One-line subtitle for a saved-view hit, mirroring the concept copy
 *  on the `/views` index: filter views update from rules, reading lists
 *  track an imported CBL. */
function savedViewSubtitle(kind: SavedViewView["kind"]): string {
  switch (kind) {
    case "cbl":
      return "Reading list";
    case "filter_series":
      return "Filter view";
    default:
      return "View";
  }
}

/** Cached lists the saved-content palette categories (A4) filter over. */
export interface SavedContentSources {
  /** `/me/saved-views` — all kinds; the `views` category keeps only the
   *  user-content kinds (`filter_series` / `cbl`) and drops the `system`
   *  rails + `collection` rows (collections get their own category). */
  savedViews: SavedViewView[];
  /** `/me/collections` — collection rows, including the built-in Want to
   *  Read. */
  collections: SavedViewView[];
  /** `/me/pages` — the user's custom + system pages. */
  pages: PageView[];
}

export interface SavedContentHits {
  views: SearchHit[];
  collections: SearchHit[];
  pages: SearchHit[];
}

const EMPTY_SAVED_CONTENT: SavedContentHits = {
  views: [],
  collections: [],
  pages: [],
};

/** Build the `views` / `collections` / `pages` palette hits by matching
 *  cached lists on name (A4). `needle` MUST be pre-lowercased; pass `""`
 *  to short-circuit to empty groups. Pure + exported so the
 *  categorization rules are unit-tested directly (no hook render). The
 *  caller slices each array to its per-category cap and reads `.length`
 *  for the category total. */
export function buildSavedContentHits(
  needle: string,
  sources: SavedContentSources,
): SavedContentHits {
  if (!needle) return EMPTY_SAVED_CONTENT;
  const views: SearchHit[] = sources.savedViews
    .filter(
      (v) =>
        (v.kind === "filter_series" || v.kind === "cbl") &&
        v.name.toLowerCase().includes(needle),
    )
    .map((v) => ({
      kind: "views" as const,
      id: v.id,
      title: v.name,
      subtitle: savedViewSubtitle(v.kind),
      href: `/views/${v.id}`,
      icon: ListChecks,
    }));
  const collections: SearchHit[] = sources.collections
    .filter((c) => c.name.toLowerCase().includes(needle))
    .map((c) => {
      const isWantToRead = c.system_key === WANT_TO_READ_KEY;
      return {
        kind: "collections" as const,
        id: c.id,
        title: c.name,
        subtitle: isWantToRead ? "Built-in collection" : "Collection",
        href: `/views/${isWantToRead ? "want-to-read" : c.id}`,
        icon: Folder,
      };
    });
  const pages: SearchHit[] = sources.pages
    .filter((p) => p.name.toLowerCase().includes(needle))
    .map((p) => ({
      kind: "pages" as const,
      id: p.id,
      title: p.name,
      subtitle: "Page",
      href: `/pages/${p.slug}`,
      icon: LayoutDashboard,
    }));
  return { views, collections, pages };
}

/** Raw payload arrays for the cover-renderable categories. The
 *  `/search` page reads these so it can render proper `<SeriesCard>` /
 *  `<IssueCard>` components (cover-menu, badges, progress overlay)
 *  instead of the generic `SearchHit` layout the modal uses. */
export interface GlobalSearchPayloads {
  series: SeriesView[];
  issues: IssueSearchHit[];
}

export type GlobalSearchTotals = Record<SearchCategory, number>;

interface GlobalSearchResult {
  /** True when the query is long enough to actually run a search. */
  enabled: boolean;
  /** True if any backing query is currently fetching. */
  isLoading: boolean;
  groups: SearchGroups;
  payloads: GlobalSearchPayloads;
  categoryTotals: GlobalSearchTotals;
  total: number;
}

const EMPTY_PAYLOADS: GlobalSearchPayloads = {
  series: [],
  issues: [],
};

const EMPTY_TOTALS: GlobalSearchTotals = {
  series: 0,
  issues: 0,
  markers: 0,
  people: 0,
  views: 0,
  collections: 0,
  pages: 0,
};

/** Build the URL a person hit links to.
 *
 *  Preferred: `/creators/<slug>` — the M8 detail page. Returns a
 *  proper entity page with per-role series rails, replacing the
 *  earlier any-role library-grid URL. When the backend hit is missing
 *  a `slug` (a freshly-scanned credit the `person` backfill hasn't
 *  caught yet), fall back to the older `?library=all&credits=<name>`
 *  shape so the user still lands somewhere useful. */
function hrefForPerson(p: { person: string; slug?: string | null }): string {
  if (p.slug) return `/creators/${encodeURIComponent(p.slug)}`;
  return `/?library=all&credits=${encodeURIComponent(p.person)}`;
}

function peopleSubtitle(roles: readonly string[], credits: number): string {
  // Show every role rather than capping at 3. Creators with many
  // roles (cover artists who also write) were losing visible roles
  // before; the truncation hid the breadth of their credits.
  const labels = roles.map(formatRole);
  const roleStr = labels.join(" · ");
  const creditStr = `${credits} ${credits === 1 ? "credit" : "credits"}`;
  return roleStr ? `${roleStr} · ${creditStr}` : creditStr;
}

function formatRole(role: string): string {
  return role
    .split("_")
    .map((s) => (s.length === 0 ? s : s[0]!.toUpperCase() + s.slice(1)))
    .join(" ");
}

/** Per-kind icon for the markers section. Mirrors the chrome icons
 *  used in the reader's marker UI so a bookmark renders with the
 *  same bookmark glyph the user originally created it with. */
const MARKER_KIND_ICON: Record<MarkerKind, LucideIcon> = {
  bookmark: Bookmark,
  note: StickyNote,
  favorite: Star,
  highlight: Highlighter,
};

const MARKER_KIND_LABEL: Record<MarkerKind, string> = {
  bookmark: "Bookmark",
  note: "Note",
  favorite: "Favorite",
  highlight: "Highlight",
};

/** Turn a marker search hit into the generic `SearchHit` shape the
 *  modal + `/search` rails consume. The hit's link jumps to the
 *  reader at the right page, mirroring the `buildJumpHref` pattern
 *  on the `/bookmarks` page. Falls back to a relative no-op when
 *  the row is missing slugs (defensive — `/me/markers/search`
 *  hydrates them server-side, but a stale cache row shouldn't
 *  navigate the user to "/"). */
function markerToSearchHit(m: MarkerSearchHit): SearchHit {
  const seriesLabel = m.series_name ?? "Unknown series";
  const issueChunk = m.issue_number
    ? `#${m.issue_number}`
    : (m.issue_title ?? "");
  const subtitle = [
    MARKER_KIND_LABEL[m.kind] ?? "Marker",
    seriesLabel,
    issueChunk || null,
    `Page ${m.page_index + 1}`,
  ]
    .filter(Boolean)
    .join(" · ");
  const href =
    m.series_slug && m.issue_slug
      ? `${readerUrl(m.series_slug, m.issue_slug)}?page=${m.page_index}`
      : "#";
  return {
    kind: "markers" as const,
    id: m.id,
    title:
      m.issue_title || (m.issue_number ? `#${m.issue_number}` : seriesLabel),
    subtitle,
    href,
    thumbUrl: pageBytesUrl(m.issue_id, m.page_index),
    issueId: m.issue_id,
    pageIndex: m.page_index,
    region: m.region ?? null,
    snippet: m.snippet ?? null,
    icon: MARKER_KIND_ICON[m.kind] ?? Bookmark,
  };
}

/**
 * Fan-out search hook. Series, issues, markers, and people each have
 * their own backend query; views / collections / pages (A4) are matched
 * client-side over already-cached lists (`buildSavedContentHits`). The
 * hook merges them all into `SearchHit` groups plus the raw payload
 * arrays used by the rails on `/search`. When a new category lands, plug
 * another source in here and map its rows into `SearchHit`s with the
 * matching `kind` — the modal and `/search` pick them up automatically
 * once the category is added to `SEARCH_CATEGORIES`.
 */
export function useGlobalSearch(
  rawQuery: string,
  opts: GlobalSearchOpts = {},
): GlobalSearchResult {
  const query = rawQuery.trim();
  const enabled = query.length >= MIN_QUERY_LEN;
  // Resolve per-backend limit: explicit override wins (modal passes 5),
  // otherwise ask each backend for its server-side max so the rails
  // don't silently clamp.
  const seriesLimit = opts.perCategory ?? BACKEND_MAX.series;
  const issuesLimit = opts.perCategory ?? BACKEND_MAX.issues;
  const markersLimit = opts.perCategory ?? BACKEND_MAX.markers;
  const peopleLimit = opts.perCategory ?? BACKEND_MAX.people;

  const series = useSeriesList(
    enabled
      ? { q: query, limit: seriesLimit, ...(opts.seriesFilters ?? {}) }
      : {},
  );
  const issues = useIssueSearch(
    enabled ? { q: query, limit: issuesLimit } : {},
  );
  const markers = useMarkerSearch(
    enabled ? { q: query, limit: markersLimit } : {},
  );
  const people = usePeopleSearch(
    enabled ? { q: query, limit: peopleLimit } : {},
  );

  // Saved-content sources (A4). These hooks read already-cached,
  // per-user-bounded lists (`/me/saved-views`, `/me/collections`,
  // `/me/pages`), so the palette can match a view / collection / page
  // by name without a new backend. They're filtered client-side below;
  // calling them unconditionally just shares the existing cache.
  const savedViews = useSavedViews();
  const collections = useCollections();
  const pages = useMePages();
  const savedContentLimit = opts.perCategory ?? SAVED_CONTENT_MAX;

  const seriesItems = useMemo<SeriesView[]>(
    () => (enabled ? (series.data?.items ?? []) : []),
    [enabled, series.data],
  );
  const issueItems = useMemo<IssueSearchHit[]>(
    () => (enabled ? (issues.data?.items ?? []) : []),
    [enabled, issues.data],
  );

  // Lower-cased query for the client-side name match. Empty when the
  // search isn't enabled so the helper short-circuits to empty groups.
  const needle = enabled ? query.toLowerCase() : "";

  // Saved-content hits (views / collections / pages), name-matched over
  // the cached lists. Unsliced here so `categoryTotals` can report the
  // true match count; the `groups` memo applies the per-category cap.
  const savedContent = useMemo<SavedContentHits>(
    () =>
      buildSavedContentHits(needle, {
        savedViews: savedViews.data?.items ?? [],
        collections: collections.data ?? [],
        pages: pages.data ?? [],
      }),
    [needle, savedViews.data, collections.data, pages.data],
  );

  const groups = useMemo<SearchGroups>(() => {
    if (!enabled) return EMPTY_SEARCH_GROUPS;
    const seriesHits: SearchHit[] = seriesItems.map((s) => ({
      kind: "series" as const,
      id: s.id,
      title: s.name,
      subtitle:
        [s.publisher, s.year != null ? String(s.year) : null]
          .filter(Boolean)
          .join(" · ") || null,
      href: seriesUrl(s),
      thumbUrl: s.cover_url,
      snippet: s.snippet ?? null,
    }));
    const issueHits: SearchHit[] = issueItems.map((i) => ({
      kind: "issues" as const,
      id: i.id,
      title:
        i.title ??
        (i.number != null ? `#${i.number}` : i.series_name) ??
        "Untitled",
      subtitle:
        [i.series_name, i.number != null ? `#${i.number}` : null]
          .filter(Boolean)
          .join(" · ") || null,
      href: issueUrl(i),
      thumbUrl: i.cover_url,
      snippet: i.snippet ?? null,
    }));
    const markerHits: SearchHit[] = (markers.data?.items ?? []).map((m) =>
      markerToSearchHit(m),
    );
    const peopleHits: SearchHit[] = (people.data?.items ?? []).map((p) => ({
      kind: "people" as const,
      id: p.person,
      title: p.person,
      subtitle: peopleSubtitle(p.roles, p.credit_count),
      href: hrefForPerson(p),
      icon: User,
    }));
    return {
      ...EMPTY_SEARCH_GROUPS,
      series: seriesHits,
      issues: issueHits,
      markers: markerHits,
      people: peopleHits,
      views: savedContent.views.slice(0, savedContentLimit),
      collections: savedContent.collections.slice(0, savedContentLimit),
      pages: savedContent.pages.slice(0, savedContentLimit),
    };
  }, [
    enabled,
    seriesItems,
    issueItems,
    markers.data,
    people.data,
    savedContent,
    savedContentLimit,
  ]);

  const payloads = useMemo<GlobalSearchPayloads>(() => {
    if (!enabled) return EMPTY_PAYLOADS;
    return { series: seriesItems, issues: issueItems };
  }, [enabled, seriesItems, issueItems]);

  const categoryTotals = useMemo<GlobalSearchTotals>(() => {
    if (!enabled) return EMPTY_TOTALS;
    return {
      series: series.data?.total ?? seriesItems.length,
      issues: issueItems.length,
      markers: markers.data?.items.length ?? 0,
      people: people.data?.items.length ?? 0,
      views: savedContent.views.length,
      collections: savedContent.collections.length,
      pages: savedContent.pages.length,
    };
  }, [
    enabled,
    series.data?.total,
    seriesItems.length,
    issueItems.length,
    markers.data?.items.length,
    people.data?.items.length,
    savedContent,
  ]);

  // Future: aggregate `isFetching` across each backend so the spinner is
  // honest about what's still pending.
  const isLoading =
    enabled &&
    (series.isFetching ||
      issues.isFetching ||
      markers.isFetching ||
      people.isFetching);

  const total = Object.values(categoryTotals).reduce((sum, n) => sum + n, 0);

  return {
    enabled,
    isLoading,
    groups,
    payloads,
    categoryTotals,
    total,
  };
}
