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
 * Field::CommunityRating doesn't exist on the saved-views DSL today, so
 * a non-default `ratingRange` falls onto `droppedFacets` — the UI
 * surfaces a toast and proceeds with the rest of the conditions.
 */
import type { FilterBuilderState } from "@/components/filters/filter-builder";
import type { Condition, Field } from "@/lib/api/types";
import {
  CREDIT_ROLES,
  type CreditKey,
  type CreditState,
  type MetadataCompletenessTier,
  RATING_MIN,
  RATING_MAX,
} from "./library-grid-filters";

export type LibraryGridFilterSnapshot = {
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
  if (s.metadataCompleteness) {
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

  // Rating: the library grid filters on `user_rating` (per-user
  // community rating). The saved-views DSL doesn't have a field for it
  // yet, so we drop it with a user-visible note rather than silently
  // omit it.
  if (s.ratingRange) {
    const [min, max] = s.ratingRange;
    if (min > RATING_MIN || max < RATING_MAX) {
      dropped.push("Rating");
    }
  }

  return {
    state: {
      name: `Library filter — ${today}`,
      matchMode: "all",
      conditions,
    },
    droppedFacets: dropped,
  };
}
