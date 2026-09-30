/**
 * Duplicates-page mutations (WP-3.3). Re-exported from `./index` so
 * callers import from `"@/lib/api/mutations"` as usual.
 *
 * - `PUT    /series/{s}/issues/{i}/duplicate-decision` `{decision}` —
 *   `keep` marks the copy reviewed; `remove` soft-removes it (sticky
 *   across rescans until cleared or restored from the Removed tab).
 * - `DELETE /series/{s}/issues/{i}/duplicate-decision` — clear the
 *   decision and undo a duplicate soft-remove.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type { DuplicateDecision } from "../types";
import { useApiMutation } from "./_core";

type IssueRef = { seriesSlug: string; issueSlug: string };

function decisionPath({ seriesSlug, issueSlug }: IssueRef) {
  return `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(
    issueSlug,
  )}/duplicate-decision`;
}

function useInvalidateDuplicates(libraryId: string) {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: queryKeys.duplicatesAll(libraryId) });
    qc.invalidateQueries({ queryKey: queryKeys.removed(libraryId) });
  };
}

export function useSetDuplicateDecision(libraryId: string) {
  const invalidate = useInvalidateDuplicates(libraryId);
  return useApiMutation<null, IssueRef & { decision: DuplicateDecision }>(
    ({ decision, ...ref }) => ({
      path: decisionPath(ref),
      method: "PUT",
      body: { decision },
    }),
    {
      successMessage: (_data, { decision }) =>
        decision === "remove"
          ? "Copy removed — restore it from the Removed tab"
          : "Marked as kept",
      onSuccess: invalidate,
    },
  );
}

export function useClearDuplicateDecision(libraryId: string) {
  const invalidate = useInvalidateDuplicates(libraryId);
  return useApiMutation<null, IssueRef>(
    (ref) => ({ path: decisionPath(ref), method: "DELETE" }),
    {
      successMessage: "Decision cleared",
      onSuccess: invalidate,
    },
  );
}
