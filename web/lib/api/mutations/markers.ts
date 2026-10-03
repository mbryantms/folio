/**
 * Marker mutations (bookmark / note / favorite / highlight).
 *
 * Extracted from `mutations/index.ts` for the reader bundle budget
 * (WP-4.4): the reader imports these directly from
 * `@/lib/api/mutations/markers` so it doesn't pull the whole mutations
 * barrel (every admin / library hook) into its first-load JS — Turbopack
 * doesn't tree-shake unused exports out of a module that is reachable.
 * `index.ts` re-exports `*` from here, so other callers keep importing
 * from `"@/lib/api/mutations"`.
 */
import { useQueryClient } from "@tanstack/react-query";

import { markerToRestoreItem } from "@/lib/markers/recreate";

import type {
  CreateMarkerReq,
  MarkerBulkDeleteResp,
  MarkerView,
  RestoreMarkersResp,
  UpdateMarkerReq,
} from "../types";
import { queryKeys } from "../query-keys";
import { useApiMutation } from "./_core";

/** Create a marker — bookmark / note / favorite / highlight. The
 *  reader cover-menu and `b` / `n` / `h` keybinds chain here. On
 *  success the per-issue + global feed caches are invalidated so the
 *  overlay and `/bookmarks` page refresh together.
 *
 *  No toast on success — reader keybind call sites wrap with
 *  kind-specific toasts ("Bookmarked page X", "Starred page X"); a
 *  generic "Marker created" would compete. */
export function useCreateMarker() {
  const qc = useQueryClient();
  return useApiMutation<MarkerView, CreateMarkerReq>(
    (body) => ({ path: "/me/markers", method: "POST", body }),
    {
      onSuccess: (_data, input) => {
        qc.invalidateQueries({
          queryKey: ["markers", "issue", input.issue_id],
        });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        // Sidebar badge — only create + delete change the total, so we
        // skip the count invalidation in `useUpdateMarker`.
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** Edit a marker's body / color / region / selection. Per-kind
 *  invariants are enforced server-side (e.g. a note body can't be
 *  cleared). `issueId` keys the per-issue invalidation. */
export function useUpdateMarker(id: string, issueId: string) {
  const qc = useQueryClient();
  return useApiMutation<MarkerView, UpdateMarkerReq>(
    (body) => ({ path: `/me/markers/${id}`, method: "PATCH", body }),
    {
      // Tailor the toast for the most common single-field toggles so
      // the action is unambiguous. Falls back to "Saved" for editor
      // submits (body / region / tags / multiple-field updates).
      successMessage: (_data, input) => {
        const keys = Object.keys(input);
        if (keys.length === 1 && keys[0] === "is_favorite") {
          return input.is_favorite
            ? "Added to favorites"
            : "Removed from favorites";
        }
        return "Saved";
      },
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "issue", issueId] });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        // Tag edits change the rollup but not the count — invalidate
        // tags specifically (count stays stable so we skip that key).
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** `silent: true` suppresses the default "Removed" toast for callers
 *  that compose their own (a) Reader keybinds emit kind-specific
 *  labels like "Removed bookmark on page X"; (b) every delete surface
 *  pairs the toast with an Undo action via `useRestoreMarkers`. The 8 marker-delete call sites all use
 *  `silent: true` post-M3.5 — see docs/dev/notifications-audit.md
 *  §F-8 / cleanup plan M3.5. */
export function useDeleteMarker(
  id: string,
  issueId: string,
  opts?: { silent?: boolean },
) {
  const qc = useQueryClient();
  return useApiMutation<unknown, void>(
    () => ({ path: `/me/markers/${id}`, method: "DELETE" }),
    {
      ...(opts?.silent ? {} : { successMessage: "Removed" }),
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "issue", issueId] });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** Delete a marker whose id arrives at `mutate()` time. For hot
 *  paths like the reader's `b`/`s` toggles, where the fixed-id
 *  [`useDeleteMarker`] forced re-deriving a hook per page turn and
 *  minted mutations bound to `""` whenever no marker existed on the
 *  current page. Same invalidation set. */
export function useDeleteMarkerById(
  issueId: string,
  opts?: { silent?: boolean },
) {
  const qc = useQueryClient();
  return useApiMutation<unknown, string>(
    (id) => ({ path: `/me/markers/${id}`, method: "DELETE" }),
    {
      ...(opts?.silent ? {} : { successMessage: "Removed" }),
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "issue", issueId] });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** Bulk-delete markers by id for the /bookmarks multi-select flow
 *  (audit B11). Input is the selected id list; the server caps at 500,
 *  dedups, and silently skips ids that aren't the caller's. Returns
 *  `{ deleted, not_found }`.
 *
 *  No `successMessage` — like the single delete, the call site composes
 *  its own toast with an Undo action (the marker-delete exception to the
 *  AlertDialog-confirm rule; see docs/dev/notifications-audit.md §F-8).
 *  Same invalidation set as `useDeleteMarker` minus the per-issue key
 *  (a bulk selection spans many issues). */
export function useBulkDeleteMarkers() {
  const qc = useQueryClient();
  return useApiMutation<MarkerBulkDeleteResp, string[]>(
    (markerIds) => ({
      path: "/me/markers/bulk-delete",
      method: "POST",
      body: { marker_ids: markerIds },
    }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** Undo for every marker delete (WP-8.4): re-insert the snapshots the
 *  call site captured before deleting, in ONE `POST /me/markers/restore`
 *  however many markers there are (a 500-marker bulk delete included).
 *  The server keeps each snapshot's id, created_at and page hash.
 *
 *  Silent on success, like the per-marker re-create it replaces — the
 *  restored markers reappearing is the feedback; errors still toast.
 *  Invalidates every per-issue overlay the snapshots touched plus the
 *  global feed, count and tag rollup. */
export function useRestoreMarkers() {
  const qc = useQueryClient();
  return useApiMutation<RestoreMarkersResp, MarkerView[]>(
    (snapshots) => ({
      path: "/me/markers/restore",
      method: "POST",
      body: { markers: snapshots.map(markerToRestoreItem) },
    }),
    {
      onSuccess: (_data, snapshots) => {
        for (const issueId of new Set(snapshots.map((m) => m.issue_id))) {
          qc.invalidateQueries({ queryKey: queryKeys.issueMarkers(issueId) });
        }
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}
