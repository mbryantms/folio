/**
 * WP-4.3 — manual spread controls for the double-page reader.
 *
 * - `PUT /me/issues/{id}/page-overrides` replaces the caller's whole
 *   override set for one issue. Written to the query cache
 *   optimistically so the reader re-pairs on the same frame as the tap;
 *   rolled back if the server rejects the write. No success toast — the
 *   re-paired spread is the feedback.
 * - `DELETE` resets the issue to automatic pairing (toasts on success).
 * - Both write through to the issue's offline download record, when
 *   there is one, so the offline reader pairs with the change (N-R7).
 */
import { useQueryClient } from "@tanstack/react-query";

import type { SpreadOverrides } from "@/lib/reader/spreads";
import { queryKeys } from "../queries";
import type { PageOverridesView } from "../types";
import { useApiMutation } from "./_core";

type Snapshot = { previous?: PageOverridesView };

/** Best-effort write-through to the download record (no-op when the
 *  issue is not downloaded or offline storage is unavailable). */
async function storeOffline(issueId: string, view: PageOverridesView) {
  try {
    const { getDownloadManager } = await import("@/lib/pwa/downloads");
    await getDownloadManager().noteOverrides(issueId, view);
  } catch {
    /* The live cache already has the change. */
  }
}

function useOptimisticWrite(issueId: string) {
  const qc = useQueryClient();
  const key = queryKeys.issuePageOverrides(issueId);
  return {
    async apply(next: SpreadOverrides): Promise<Snapshot> {
      await qc.cancelQueries({ queryKey: key });
      const previous = qc.getQueryData<PageOverridesView>(key);
      qc.setQueryData<PageOverridesView>(key, {
        issue_id: issueId,
        updated_at: previous?.updated_at ?? null,
        shift_pairing: next.shift_pairing,
        spread_pages: [...next.spread_pages],
        single_pages: [...next.single_pages],
      });
      return { previous };
    },
    rollback(snapshot: unknown) {
      const previous = (snapshot as Snapshot | undefined)?.previous;
      if (previous) qc.setQueryData(key, previous);
      else qc.invalidateQueries({ queryKey: key });
    },
    settle(data: PageOverridesView | null) {
      if (data) qc.setQueryData(key, data);
      else qc.invalidateQueries({ queryKey: key });
    },
  };
}

export function useSetIssuePageOverrides(issueId: string) {
  const cache = useOptimisticWrite(issueId);
  return useApiMutation<PageOverridesView, SpreadOverrides>(
    (body) => ({
      path: `/me/issues/${issueId}/page-overrides`,
      method: "PUT",
      body: {
        shift_pairing: body.shift_pairing,
        spread_pages: [...body.spread_pages],
        single_pages: [...body.single_pages],
      },
    }),
    {
      onMutate: (input) => cache.apply(input),
      onError: (_err, _input, snapshot) => cache.rollback(snapshot),
      onSuccess: (data) => {
        cache.settle(data);
        if (data) void storeOffline(issueId, data);
      },
    },
  );
}

export function useResetIssuePageOverrides(issueId: string) {
  const cache = useOptimisticWrite(issueId);
  return useApiMutation<null, void>(
    () => ({
      path: `/me/issues/${issueId}/page-overrides`,
      method: "DELETE",
    }),
    {
      successMessage: "Spread overrides cleared",
      onMutate: () =>
        cache.apply({
          shift_pairing: false,
          spread_pages: [],
          single_pages: [],
        }),
      onError: (_err, _input, snapshot) => cache.rollback(snapshot),
      onSuccess: () =>
        void storeOffline(issueId, {
          issue_id: issueId,
          shift_pairing: false,
          spread_pages: [],
          single_pages: [],
          updated_at: null,
        }),
    },
  );
}
