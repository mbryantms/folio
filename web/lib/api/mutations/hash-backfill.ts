/**
 * First-import lazy-hash mutations (WP-3.2). Re-exported from
 * `mutations/index.ts`, so callers import from `"@/lib/api/mutations"`.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type { HashBackfillStartResp } from "../types";
import { useApiMutation } from "./_core";

/**
 * `POST /libraries/{slug}/hash-backfill`. (Re)enqueues the content-hash
 * drain — scans already enqueue it, so this is the manual resume after a
 * restart abandoned one.
 */
export function useStartHashBackfill(librarySlug: string) {
  const qc = useQueryClient();
  return useApiMutation<HashBackfillStartResp, void>(
    () => ({
      path: `/libraries/${librarySlug}/hash-backfill`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        data?.enqueued
          ? `Hashing ${data.pending} file${data.pending === 1 ? "" : "s"} in the background`
          : "Nothing left to hash",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.hashBackfill(librarySlug) });
        qc.invalidateQueries({ queryKey: queryKeys.queueDepth });
      },
    },
  );
}
