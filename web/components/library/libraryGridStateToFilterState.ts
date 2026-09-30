/**
 * Translator — converts the library grid's local filter state into a
 * `FilterBuilderState` seed for `<NewFilterViewDialog>`. Pure module
 * (no React, no DOM, no fetch) so it tests cleanly under vitest and
 * carries no runtime dependency on the grid itself.
 *
 * The grid and the saved-views builder remain independent code paths;
 * this is a one-way export at the moment the user clicks "Save as
 * view…". After save, the persisted view is owned by the builder.
 *
 * WP-5.4: the grid's mode picks the view entity — series mode seeds a
 * `filter_series` view, issues mode a `filter_issues` view — and the
 * per-user rating and read-status facets now carry over (`rating
 * between`, `read_status in`). Facets with no equivalent on the target
 * entity (metadata completeness is a series rollup) fall onto
 * `droppedFacets`; the UI surfaces a toast and proceeds with the rest.
 */
import type { FilterBuilderState } from "@/components/filters/filter-builder";
import type { Condition, Field } from "@/lib/api/types";
import {
  CREDIT_ROLES,
  type CreditKey,
  type CreditState,
  type LibraryGridMode,
  type MetadataCompletenessTier,
  RATING_MIN,
  RATING_MAX,
} from "./library-grid-filters";

export type LibraryGridFilterSnapshot = {
  /** Grid mode — decides whether the seeded view lists series or
   *  issues. Omitted → series (the pre-WP-5.4 behaviour). */
  mode?: LibraryGridMode;
  /** Per-user read-status chips (`unread` / `in_progress` / `read`). */
  readStatus?: string[];
  status: string;
  metadataCompleteness: MetadataCompletenessTier | undefined;
  yearFrom: string;
  yearTo: string;
  publishers: string[];
  languages: string[];
  ageRatings: string[];
  genres: string[];
  tags: string[];
  credits: CreditState;
  characters: string[];
  teams: string[];
  locations: string[];
  ratingRange: [number, number] | null;
  trimmedQ: string;
};

export type TranslateResult = {
  state: Partial<FilterBuilderState>;
  /** Human-friendly facet labels that couldn't be expressed in the
   *  current DSL. Caller surfaces these as toast warnings. */
  droppedFacets: string[];
};

/** Map per-credit-role state keys (`writers`, `pencillers`, …) to the
 *  singular DSL `Field` ids (`writer`, `penciller`, …). */
const CREDIT_KEY_TO_FIELD: Record<CreditKey, Field> = Object.fromEntries(
  CREDIT_ROLES.map((c) => [c.key, c.role as Field]),
) as Record<CreditKey, Field>;

export function libraryGridStateToFilterBuilderState(
  s: LibraryGridFilterSnapshot,
  today: string,
): TranslateResult {
  const conditions: Condition[] = [];
  const dropped: string[] = [];
  const entity = s.mode === "issues" ? "issue" : "series";

  if (s.trimmedQ) {
    conditions.push({
      group_id: 0,
      field: "name",
      op: "contains",
      value: s.trimmedQ,
    });
  }

  if (s.status && s.status !== "any") {
    conditions.push({
      group_id: 0,
      field: "status",
      op: "is",
      value: s.status,
    });
  }

  // Completeness is a first-class saved-view field — carry it so a saved
  // "Needs metadata" worklist keeps filtering after the grid hands off.
  if (s.metadataCompleteness && entity === "issue") {
    // Series-level rollup — no issue-view equivalent.
    dropped.push("Metadata completeness");
  } else if (s.metadataCompleteness) {
    conditions.push({
      group_id: 0,
      field: "metadata_completeness",
      op: "is",
      value: s.metadataCompleteness,
    });
  }

  const yf = parseInt(s.yearFrom, 10);
  const yt = parseInt(s.yearTo, 10);
  if (Number.isFinite(yf) && Number.isFinite(yt)) {
    conditions.push({
      group_id: 0,
      field: "year",
      op: "between",
      value: [yf, yt],
    });
  } else if (Number.isFinite(yf)) {
    conditions.push({ group_id: 0, field: "year", op: "gte", value: yf });
  } else if (Number.isFinite(yt)) {
    conditions.push({ group_id: 0, field: "year", op: "lte", value: yt });
  }

  if (s.publishers.length > 0) {
    conditions.push({
      group_id: 0,
      field: "publisher",
      op: "in",
      value: s.publishers,
    });
  }
  if (s.languages.length > 0) {
    conditions.push({
      group_id: 0,
      field: "language_code",
      op: "in",
      value: s.languages,
    });
  }
  if (s.ageRatings.length > 0) {
    conditions.push({
      group_id: 0,
      field: "age_rating",
      op: "in",
      value: s.ageRatings,
    });
  }

  // Multi-valued junction-backed fields all use `includes_any`. Order
  // matches the chip rendering so the resulting builder reads top-to-
  // bottom like the active-chips row.
  const multi: ReadonlyArray<readonly [Field, string[]]> = [
    ["genres", s.genres],
    ["tags", s.tags],
    ["characters", s.characters],
    ["teams", s.teams],
    ["locations", s.locations],
  ];
  for (const [field, vals] of multi) {
    if (vals.length > 0) {
      conditions.push({
        group_id: 0,
        field,
        op: "includes_any",
        value: vals,
      });
    }
  }

  for (const c of CREDIT_ROLES) {
    const vals = s.credits[c.key];
    if (vals && vals.length > 0) {
      conditions.push({
        group_id: 0,
        field: CREDIT_KEY_TO_FIELD[c.key],
        op: "includes_any",
        value: vals,
      });
    }
  }

  // Read status: every state selected (or none) is a no-op on the grid,
  // so only a strict subset becomes a condition.
  const readStatus = s.readStatus ?? [];
  if (readStatus.length > 0 && readStatus.length < 3) {
    conditions.push({
      group_id: 0,
      field: "read_status",
      op: "in",
      value: readStatus,
    });
  }

  // Rating: the grid filters on the caller's own `user_rating` — the
  // series rating in series mode, the issue rating in issues mode — which
  // is exactly the DSL's `rating` field on each entity (WP-5.4).
  if (s.ratingRange) {
    const [min, max] = s.ratingRange;
    if (min > RATING_MIN || max < RATING_MAX) {
      conditions.push({
        group_id: 0,
        field: "rating",
        op: "between",
        value: [min, max],
      });
    }
  }

  return {
    state: {
      name: `Library filter — ${today}`,
      entity,
      matchMode: "all",
      conditions,
    },
    droppedFacets: dropped,
  };
}
