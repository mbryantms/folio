/**
 * Relationship-suggestion review mutations (WP-7.3). Re-exported from
 * `./index` so callers import from `"@/lib/api/mutations"` as usual.
 *
 * - `POST /admin/relationship-suggestions/{id}/accept` `{kind?}`
 * - `POST /admin/relationship-suggestions/{id}/reject`
 * - `POST /admin/relationship-suggestions/{id}/reopen` (rejected → pending)
 * - `POST /admin/relationship-suggestions/bulk-accept`
 *   `{ids}` or `{bucket: "high", library_id?}` — one batch, one audit row
 * - `POST /admin/relationship-suggestions/bulk-reject` `{ids}`
 * - `POST /admin/relationship-suggestions/run?library_id=`
 *
 * Accepting creates a relationship pair, so it also invalidates both
 * series' "Related" blocks and the similar-series rails (accepted edges are
 * a WP-7.4 similarity signal).
 */
import { useQueryClient, type QueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type {
  AcceptRelationshipSuggestionResp,
  BulkAcceptRelationshipSuggestionsReq,
  BulkReviewRelationshipSuggestionsResp,
  RelationshipKind,
  RelationshipSuggestionView,
  ReopenRelationshipSuggestionResp,
  RunRelationshipSuggestionsResp,
} from "../types";
import { useApiMutation } from "./_core";

const BASE = "/admin/relationship-suggestions";

/** Every suggestion list: the admin queue and the per-series chips. */
function invalidateSuggestionLists(qc: QueryClient) {
  qc.invalidateQueries({ queryKey: queryKeys.relationshipSuggestionsAll });
  qc.invalidateQueries({
    predicate: (q) =>
      q.queryKey[0] === "series" &&
      q.queryKey[2] === "relationship-suggestions",
  });
}

/** After edges were created: the "Related" blocks of the given series (or
 *  every series when unknown, e.g. a bucket-mode bulk accept) and the
 *  similar-series rails. */
function invalidateRelationships(qc: QueryClient, slugs: string[] | null) {
  if (slugs) {
    for (const slug of slugs) {
      qc.invalidateQueries({ queryKey: queryKeys.seriesRelationships(slug) });
    }
  } else {
    qc.invalidateQueries({
      predicate: (q) =>
        q.queryKey[0] === "series" && q.queryKey[2] === "relationships",
    });
  }
  // `["similar"]` covers the per-series rail and the home rail.
  qc.invalidateQueries({ queryKey: ["similar"] });
}

function seriesLabel(s: { name: string; year?: number | null }): string {
  return s.year != null ? `${s.name} (${s.year})` : s.name;
}

export type AcceptSuggestionInput = {
  id: string;
  /** Accept as a different kind (read "from `kind` to") → `modified`. */
  kind?: RelationshipKind;
};

export function useAcceptRelationshipSuggestion() {
  const qc = useQueryClient();
  return useApiMutation<
    AcceptRelationshipSuggestionResp,
    AcceptSuggestionInput
  >(
    ({ id, kind }) => ({
      path: `${BASE}/${encodeURIComponent(id)}/accept`,
      method: "POST",
      body: kind ? { kind } : {},
    }),
    {
      successMessage: (data) =>
        data
          ? `Linked ${seriesLabel(data.suggestion.from_series)} → ${seriesLabel(data.suggestion.to_series ?? data.suggestion.to_arc ?? { name: "story arc" })}`
          : "Suggestion accepted",
      onSuccess: (data) => {
        invalidateSuggestionLists(qc);
        invalidateRelationships(
          qc,
          data
            ? [
                data.suggestion.from_series.slug,
                ...(data.suggestion.to_series
                  ? [data.suggestion.to_series.slug]
                  : []),
              ]
            : null,
        );
      },
    },
  );
}

export function useRejectRelationshipSuggestion() {
  const qc = useQueryClient();
  return useApiMutation<RelationshipSuggestionView, { id: string }>(
    ({ id }) => ({
      path: `${BASE}/${encodeURIComponent(id)}/reject`,
      method: "POST",
    }),
    {
      successMessage: "Suggestion rejected — it won't be suggested again",
      onSuccess: () => invalidateSuggestionLists(qc),
    },
  );
}

export function useReopenRelationshipSuggestion() {
  const qc = useQueryClient();
  return useApiMutation<ReopenRelationshipSuggestionResp, { id: string }>(
    ({ id }) => ({
      path: `${BASE}/${encodeURIComponent(id)}/reopen`,
      method: "POST",
    }),
    {
      successMessage: "Rejection cleared — the suggestion is pending again",
      onSuccess: () => invalidateSuggestionLists(qc),
    },
  );
}

/** Human summary of a bulk batch ("Accepted 12 · 2 skipped · 40 left"). */
export function bulkSummary(
  verb: "Accepted" | "Rejected",
  data: BulkReviewRelationshipSuggestionsResp | null,
): string {
  if (!data) return `${verb} suggestions`;
  const n = data.succeeded.length;
  const parts = [`${verb} ${n} suggestion${n === 1 ? "" : "s"}`];
  if (data.failed.length > 0) parts.push(`${data.failed.length} skipped`);
  if (data.remaining != null && data.remaining > 0) {
    parts.push(
      `${data.remaining} still pending — run again for the next batch`,
    );
  }
  return parts.join(" · ");
}

export function useBulkAcceptRelationshipSuggestions() {
  const qc = useQueryClient();
  return useApiMutation<
    BulkReviewRelationshipSuggestionsResp,
    BulkAcceptRelationshipSuggestionsReq
  >((body) => ({ path: `${BASE}/bulk-accept`, method: "POST", body }), {
    successMessage: (data) => bulkSummary("Accepted", data),
    onSuccess: () => {
      invalidateSuggestionLists(qc);
      invalidateRelationships(qc, null);
    },
  });
}

export function useBulkRejectRelationshipSuggestions() {
  const qc = useQueryClient();
  return useApiMutation<
    BulkReviewRelationshipSuggestionsResp,
    { ids: string[] }
  >((body) => ({ path: `${BASE}/bulk-reject`, method: "POST", body }), {
    successMessage: (data) => bulkSummary("Rejected", data),
    onSuccess: () => invalidateSuggestionLists(qc),
  });
}

/** Queue an engine run for one library (or every library when null). */
export function useRunRelationshipSuggestions() {
  const qc = useQueryClient();
  return useApiMutation<
    RunRelationshipSuggestionsResp,
    { libraryId: string | null }
  >(
    ({ libraryId }) => ({
      path: libraryId
        ? `${BASE}/run?library_id=${encodeURIComponent(libraryId)}`
        : `${BASE}/run`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        data && data.enqueued.length === 0 && data.already_queued.length > 0
          ? "A suggestion run is already queued"
          : "Suggestion run queued — new suggestions appear when it finishes",
      onSuccess: () => {
        // The run is async; refetch shortly after so a quick run shows up.
        setTimeout(() => invalidateSuggestionLists(qc), 3000);
      },
    },
  );
}
