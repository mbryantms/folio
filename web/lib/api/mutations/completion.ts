/**
 * Specials-in-completion preference (remediation B9): whether annuals,
 * one-shots, specials and TPBs count toward a series' collection
 * completeness. An account setting (`PATCH /me/preferences`) plus a
 * per-series override (`PUT /me/series/{id}/completion-prefs`). Both
 * change viewer-scoped figures on every series surface — card dots, the
 * series page's Collection badge and tab, and `collection_completeness`
 * filter results — so both hooks invalidate the same set.
 *
 * Deliberately not re-exported from the `mutations` barrel: the reader
 * imports the barrel and its first-load budget has no room for this.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import { queryKeyPrefixes } from "../query-key-prefixes";
import type {
  MeView,
  SeriesCompletionPrefsView,
  SpecialsCompletion,
} from "../types";
import { useApiMutation } from "./_core";

/** Invalidate every cached surface that renders series cards (the
 *  collection dot) or a completeness verdict. Server-rendered pages
 *  (the series page itself) need a `router.refresh()` on top. */
export function invalidateCompletenessViews(
  qc: ReturnType<typeof useQueryClient>,
) {
  for (const queryKey of Object.values(queryKeyPrefixes)) {
    qc.invalidateQueries({ queryKey });
  }
}

/** Account setting: count annuals and specials toward series completion. */
export function useSetCountSpecialsInCompleteness() {
  const qc = useQueryClient();
  return useApiMutation<MeView, boolean>(
    (value) => ({
      path: "/me/preferences",
      method: "PATCH",
      body: { count_specials_in_completeness: value },
    }),
    {
      successMessage: (_data, value) =>
        value
          ? "Annuals and specials now count toward completion"
          : "Only the main run counts toward completion",
      onSuccess: (data) => {
        if (data) qc.setQueryData(queryKeys.me, data);
        invalidateCompletenessViews(qc);
      },
    },
  );
}

const MODE_MESSAGES: Record<SpecialsCompletion, string> = {
  inherit: "This series follows your completion setting",
  include: "Specials count toward this series' completion",
  exclude: "Specials no longer count toward this series' completion",
};

/** Per-series override: `inherit` (follow the account setting),
 *  `include` or `exclude`. */
export function useSetSeriesSpecialsCompletion(seriesId: string) {
  const qc = useQueryClient();
  return useApiMutation<SeriesCompletionPrefsView, SpecialsCompletion>(
    (mode) => ({
      path: `/me/series/${encodeURIComponent(seriesId)}/completion-prefs`,
      method: "PUT",
      body: { count_specials: mode },
    }),
    {
      successMessage: (_data, mode) => MODE_MESSAGES[mode],
      onSuccess: () => invalidateCompletenessViews(qc),
    },
  );
}
