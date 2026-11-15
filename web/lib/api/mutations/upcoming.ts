/**
 * Upcoming-releases admin mutations (upcoming-releases 1.0). Re-exported
 * from `./index` so callers import from `"@/lib/api/mutations"` as usual.
 *
 * - `POST /admin/upcoming/sync` — queue a release-calendar list pass.
 * - `POST /admin/integrations/mylar/token` — create or replace the Mylar
 *   ingest token; the plaintext comes back once.
 * - `DELETE /admin/integrations/mylar/token?purge=` — revoke it.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type { MylarTokenResp, UpcomingSyncResp } from "../types";
import { useApiMutation } from "./_core";

/** Ask the server to list Metron's release dates now. The pass runs on a
 *  worker, so the status is refetched a few seconds later. */
export function useSyncUpcomingCalendar() {
  const qc = useQueryClient();
  return useApiMutation<UpcomingSyncResp, void>(
    () => ({ path: "/admin/upcoming/sync", method: "POST" }),
    {
      successMessage: (data) =>
        data && !data.queued
          ? "A sync is already queued or running"
          : "Sync queued — the calendar updates when it finishes",
      onSuccess: () => {
        setTimeout(
          () =>
            qc.invalidateQueries({ queryKey: queryKeys.adminUpcomingStatus }),
          4000,
        );
      },
    },
  );
}

/** Create (or replace) the Mylar ingest token. The response carries the
 *  plaintext, which the caller shows once; no success toast, the dialog
 *  that shows the token is the confirmation. */
export function useCreateMylarToken() {
  const qc = useQueryClient();
  return useApiMutation<MylarTokenResp, void>(
    () => ({ path: "/admin/integrations/mylar/token", method: "POST" }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.adminUpcomingStatus });
      },
    },
  );
}

/** Revoke the Mylar ingest token; `purge` also deletes the stored
 *  snapshot. */
export function useRevokeMylarToken() {
  const qc = useQueryClient();
  return useApiMutation<unknown, { purge: boolean }>(
    ({ purge }) => ({
      path: `/admin/integrations/mylar/token${purge ? "?purge=true" : ""}`,
      method: "DELETE",
    }),
    {
      successMessage: (_data, { purge }) =>
        purge
          ? "Token revoked and the stored snapshot deleted"
          : "Token revoked",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.adminUpcomingStatus });
      },
    },
  );
}
