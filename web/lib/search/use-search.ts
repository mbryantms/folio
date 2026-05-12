"use client";

import { User } from "lucide-react";
import { useMemo } from "react";

import {
  useIssueSearch,
  usePeopleSearch,
  useSeriesList,
} from "@/lib/api/queries";
import type { IssueSearchHit, SeriesView } from "@/lib/api/types";
import { issueUrl, seriesUrl } from "@/lib/urls";

import {
  EMPTY_SEARCH_GROUPS,
  totalHits,
  type SearchGroups,
  type SearchHit,
} from "./types";

const MIN_QUERY_LEN = 2;

interface GlobalSearchOpts {
  /** Soft per-category cap. The modal passes a small number (~5) so the
   *  dropdown doesn't grow unbounded; the full search page passes a
   *  bigger one. The hook only forwards this to backends that accept a
   *  `limit` — anything that returns a small fixed top-N stays as-is. */
  perCategory?: number;
}

/** Raw payload arrays for the cover-renderable categories. The
 *  `/search` page reads these so it can render proper `<SeriesCard>` /
 *  `<IssueCard>` components (cover-menu, badges, progress overlay)
 *  instead of the generic `SearchHit` layout the modal uses. */
export interface GlobalSearchPayloads {
  series: SeriesView[];
  issues: IssueSearchHit[];
}

interface GlobalSearchResult {
  /** True when the query is long enough to actually run a search. */
  enabled: boolean;
  /** True if any backing query is currently fetching. */
  isLoading: boolean;
  groups: SearchGroups;
  payloads: GlobalSearchPayloads;
  total: number;
}

const EMPTY_PAYLOADS: GlobalSearchPayloads = {
  series: [],
  issues: [],
};

/** Map a credit role to the matching `SeriesListFilters` CSV facet so
 *  clicking a person hit lands on a series list pre-filtered to their
 *  work. People who only show up under roles the library grid doesn't
 *  expose (only `writer` / `penciller` / etc. are facets today) fall
 *  back to the writers facet which still ranks the person's flagship
 *  work near the top. */
function hrefForPerson(p: {
  person: string;
  roles: readonly string[];
}): string {
  const FACET_BY_ROLE: Record<string, string> = {
    writer: "writers",
    penciller: "pencillers",
    inker: "inkers",
    colorist: "colorists",
    letterer: "letterers",
    cover_artist: "cover_artists",
    editor: "editors",
    translator: "translators",
  };
  const facet =
    p.roles.map((r) => FACET_BY_ROLE[r]).find((v) => v !== undefined) ??
    "writers";
  return `/?library=all&${facet}=${encodeURIComponent(p.person)}`;
}

function peopleSubtitle(roles: readonly string[], credits: number): string {
  const labels = roles.map(formatRole).slice(0, 3);
  const roleStr = labels.join(", ");
  const creditStr = `${credits} ${credits === 1 ? "credit" : "credits"}`;
  return roleStr ? `${roleStr} · ${creditStr}` : creditStr;
}

function formatRole(role: string): string {
  return role
    .split("_")
    .map((s) => (s.length === 0 ? s : s[0]!.toUpperCase() + s.slice(1)))
    .join(" ");
}

/**
 * Fan-out search hook. Series, issues, and people each have their own
 * backend query; the hook merges them into `SearchHit` groups plus the
 * raw payload arrays used by the rails on `/search`. When a new
 * category backend lands, plug another hook call here and map its rows
 * into `SearchHit`s with the matching `kind` — the modal and `/search`
 * pick them up automatically once the category is added to
 * `SEARCH_CATEGORIES`.
 */
export function useGlobalSearch(
  rawQuery: string,
  opts: GlobalSearchOpts = {},
): GlobalSearchResult {
  const query = rawQuery.trim();
  const enabled = query.length >= MIN_QUERY_LEN;
  const limit = opts.perCategory;

  const series = useSeriesList(
    enabled ? { q: query, ...(limit !== undefined ? { limit } : {}) } : {},
  );
  const issues = useIssueSearch(
    enabled ? { q: query, ...(limit !== undefined ? { limit } : {}) } : {},
  );
  const people = usePeopleSearch(
    enabled ? { q: query, ...(limit !== undefined ? { limit } : {}) } : {},
  );

  const seriesItems = useMemo<SeriesView[]>(
    () => (enabled ? (series.data?.items ?? []) : []),
    [enabled, series.data],
  );
  const issueItems = useMemo<IssueSearchHit[]>(
    () => (enabled ? (issues.data?.items ?? []) : []),
    [enabled, issues.data],
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
    }));
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
      people: peopleHits,
    };
  }, [enabled, seriesItems, issueItems, people.data]);

  const payloads = useMemo<GlobalSearchPayloads>(() => {
    if (!enabled) return EMPTY_PAYLOADS;
    return { series: seriesItems, issues: issueItems };
  }, [enabled, seriesItems, issueItems]);

  // Future: aggregate `isFetching` across each backend so the spinner is
  // honest about what's still pending.
  const isLoading =
    enabled && (series.isFetching || issues.isFetching || people.isFetching);

  return {
    enabled,
    isLoading,
    groups,
    payloads,
    total: totalHits(groups),
  };
}
