/**
 * Upcoming-releases admin mutations (upcoming-releases 1.0). Re-exported
 * from `./index` so callers import from `"@/lib/api/mutations"` as usual.
 *
 * - `POST /admin/upcoming/sync` — queue a release-calendar list pass.
 * - `POST /admin/integrations/mylar/token` — create or replace the Mylar
 *   ingest token; the plaintext comes back once.
 * - `DELETE /admin/integrations/mylar/token?purge=` — revoke it.
 * - `POST /admin/integrations/mylar/test` — check the stored Mylar
 *   address and key.
 * - `POST /admin/integrations/mylar/requests` — queue a reading list's
 *   missing entries for adding to Mylar.
 * - `POST /admin/integrations/mylar/search` — search the wanted list now.
 * - `POST /admin/integrations/mylar/series/{id}/recheck|refresh` and
 *   `POST /admin/integrations/mylar/issues/{id}/unwant`.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import { mylarAddMessage } from "@/lib/cbl/mylar";

import type {
  MylarActionResp,
  MylarAddReq,
  MylarAddSeriesReq,
  MylarAddSummary,
  MylarTestResp,
  MylarTokenResp,
  MylarWantedCheck,
  UpcomingSyncResp,
} from "../types";
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
        qc.invalidateQueries({ queryKey: queryKeys.adminMylarActivity });
        qc.invalidateQueries({ queryKey: ["upcoming"] });
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
        qc.invalidateQueries({ queryKey: queryKeys.adminMylarActivity });
        qc.invalidateQueries({ queryKey: ["upcoming"] });
      },
    },
  );
}

/** Check the stored Mylar address and API key. The caller shows the
 *  result inline, so there is no toast on success. */
export function useTestMylar() {
  return useApiMutation<MylarTestResp, void>(() => ({
    path: "/admin/integrations/mylar/test",
    method: "POST",
  }));
}

/** Queue missing entries of a reading list for adding to Mylar. */
export function useAddToMylar() {
  const qc = useQueryClient();
  return useApiMutation<MylarAddSummary, MylarAddReq>(
    (body) => ({
      path: "/admin/integrations/mylar/requests",
      method: "POST",
      body,
    }),
    {
      successMessage: (data) =>
        data ? mylarAddMessage(data) : "Sent to Mylar's queue",
      successVariant: (data) =>
        data && data.series_requested + data.issues_requested === 0
          ? "info"
          : "success",
      onSuccess: (_data, body) => {
        qc.invalidateQueries({
          queryKey: queryKeys.adminMylarListStatus(body.cbl_list_id),
        });
        // The first batch goes out within seconds; show its result.
        setTimeout(
          () =>
            qc.invalidateQueries({
              queryKey: queryKeys.adminMylarListStatus(body.cbl_list_id),
            }),
          5000,
        );
      },
    },
  );
}

/** Ask Mylar to search its indexers for the whole wanted list now. */
export function useMylarSearchNow() {
  return useApiMutation<MylarActionResp, void>(
    () => ({ path: "/admin/integrations/mylar/search", method: "POST" }),
    {
      successMessage: (data) =>
        data?.outcome === "done"
          ? "Mylar finished searching its wanted list"
          : "Mylar is searching its wanted list",
    },
  );
}

/** Rescan a series' files in Mylar, or re-read the series from ComicVine. */
export function useMylarSeriesAction() {
  return useApiMutation<
    MylarActionResp,
    { cvSeriesId: number; action: "recheck" | "refresh" }
  >(
    ({ cvSeriesId, action }) => ({
      path: `/admin/integrations/mylar/series/${cvSeriesId}/${action}`,
      method: "POST",
    }),
    {
      successMessage: (data, { action }) =>
        action === "refresh"
          ? "Mylar queued a refresh of the series"
          : data?.outcome === "done"
            ? "Mylar rechecked the series' files"
            : "Mylar is rechecking the series' files",
    },
  );
}

/** Set a Wanted issue back to Skipped in Mylar. */
export function useMylarUnwant() {
  const qc = useQueryClient();
  return useApiMutation<MylarActionResp, { cvIssueId: number }>(
    ({ cvIssueId }) => ({
      path: `/admin/integrations/mylar/issues/${cvIssueId}/unwant`,
      method: "POST",
    }),
    {
      successMessage: "Mylar no longer wants this issue",
      onSuccess: (_data, { cvIssueId }) => {
        qc.invalidateQueries({ queryKey: ["admin", "mylar", "list-status"] });
        // Drop the issue from a shown wanted check without asking Mylar
        // for its whole list again.
        qc.setQueryData<MylarWantedCheck>(
          queryKeys.adminMylarWantedCheck,
          (prev) => (prev ? withoutWantedIssue(prev, cvIssueId) : prev),
        );
      },
    },
  );
}

/** A wanted check with one owned-and-wanted issue taken out. */
export function withoutWantedIssue(
  check: MylarWantedCheck,
  cvIssueId: number,
): MylarWantedCheck {
  const owned = check.owned
    .map((s) => ({
      ...s,
      issues: s.issues.filter((i) => i.cv_issue_id !== cvIssueId),
    }))
    .filter((s) => s.issues.length > 0);
  const before = check.owned.reduce((n, s) => n + s.issues.length, 0);
  const after = owned.reduce((n, s) => n + s.issues.length, 0);
  const removed = before - after;
  return {
    ...check,
    wanted: check.wanted - removed,
    owned_issues: check.owned_issues - removed,
    owned_series_total:
      check.owned_series_total - (check.owned.length - owned.length),
    owned,
  };
}

/** Queue one series for Mylar's watchlist by its ComicVine id (the
 *  Upcoming page: a release whose series Mylar is not tracking). */
export function useAddSeriesToMylar() {
  return useApiMutation<MylarAddSummary, MylarAddSeriesReq>(
    (body) => ({
      path: "/admin/integrations/mylar/series",
      method: "POST",
      body,
    }),
    {
      successMessage: (data, { series_name }) =>
        !data || data.series_requested > 0
          ? `Queued ${series_name} for Mylar`
          : data.series_in_mylar > 0
            ? `${series_name} is already in Mylar`
            : `${series_name} was already requested`,
      successVariant: (data) =>
        !data || data.series_requested > 0 ? "success" : "info",
    },
  );
}
