/**
 * Upcoming-releases admin mutations (upcoming-releases 1.0). Re-exported
 * from `./index` so callers import from `"@/lib/api/mutations"` as usual.
 *
 * - `POST /admin/upcoming/sync` — queue a release-calendar list pass.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type { UpcomingSyncResp } from "../types";
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
