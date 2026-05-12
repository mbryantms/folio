/**
 * Global-search taxonomy. Adding a new category is a two-step change:
 *
 *   1. Append it to `SEARCH_CATEGORIES` (with a label).
 *   2. Wire its query function into `useGlobalSearch` and emit hits with
 *      the matching `kind`.
 *
 * The modal and the `/search` page both consume this taxonomy, so any new
 * category surfaces in both places with no UI changes.
 */

import type { ComponentType } from "react";

export type SearchCategory = "series" | "issues" | "people";

export interface SearchCategoryDef {
  /** Stable key used as the discriminator on `SearchHit.kind` and the
   *  index into `SearchGroups`. */
  key: SearchCategory;
  /** Singular display label. */
  label: string;
  /** Plural label for headings ("3 series", "12 issues", …). */
  labelPlural: string;
}

/** Display order for both the modal and the search page. */
export const SEARCH_CATEGORIES: readonly SearchCategoryDef[] = [
  { key: "series", label: "Series", labelPlural: "series" },
  { key: "issues", label: "Issue", labelPlural: "issues" },
  { key: "people", label: "Person", labelPlural: "people" },
] as const;

export interface SearchHit {
  kind: SearchCategory;
  id: string;
  title: string;
  subtitle?: string | null;
  href: string;
  thumbUrl?: string | null;
  /** Optional inline icon for hits that don't have a cover image
   *  (people, …). */
  icon?: ComponentType<{ className?: string }>;
}

export type SearchGroups = Record<SearchCategory, SearchHit[]>;

export const EMPTY_SEARCH_GROUPS: SearchGroups = {
  series: [],
  issues: [],
  people: [],
};

/** Flatten groups in display order so the modal can navigate hits with
 *  ↑/↓ regardless of which category they're in. */
export function flattenGroups(
  groups: SearchGroups,
  perCategoryCap?: number,
): SearchHit[] {
  const out: SearchHit[] = [];
  for (const def of SEARCH_CATEGORIES) {
    const slice = perCategoryCap
      ? groups[def.key].slice(0, perCategoryCap)
      : groups[def.key];
    out.push(...slice);
  }
  return out;
}

export function totalHits(groups: SearchGroups): number {
  let n = 0;
  for (const def of SEARCH_CATEGORIES) n += groups[def.key].length;
  return n;
}
