/**
 * Start Here admin mutations (Start Here + default pins M2): share an
 * admin-owned reading list with every user, and replace the featured set
 * shown in the Start Here rail. Re-exported from `index.ts`.
 */
import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";

import type { StartHereView } from "@/lib/api/types";

import { queryKeys } from "../query-keys";
import { useApiMutation } from "./_core";

/** `POST /admin/saved-views/{id}/share` — the admin's own reading list
 *  becomes a shared list every user can read (admins manage it). */
export function useShareReadingList(viewId: string) {
  const qc = useQueryClient();
  const router = useRouter();
  return useApiMutation<unknown, void>(
    () => ({ path: `/admin/saved-views/${viewId}/share`, method: "POST" }),
    {
      successMessage: "Reading list shared with everyone",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
        qc.invalidateQueries({ queryKey: ["cbl-lists"], exact: false });
        // `/views/[id]` renders the view from the server; re-run it so the
        // page (and its ⋯ menu) sees the list as shared.
        router.refresh();
      },
    },
  );
}

/** `PUT /admin/start-here` — wholesale featured order. Callers compute the
 *  new id list (append to feature, filter to unfeature, swap to reorder). */
export function useSetStartHere() {
  const qc = useQueryClient();
  const router = useRouter();
  return useApiMutation<StartHereView, { viewIds: string[]; message: string }>(
    ({ viewIds }) => ({
      path: "/admin/start-here",
      method: "PUT",
      body: { view_ids: viewIds },
    }),
    {
      successMessage: (_data, input) => input.message,
      onSuccess: (data) => {
        if (data) qc.setQueryData(queryKeys.startHere, data);
        qc.invalidateQueries({ queryKey: queryKeys.startHere });
        qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
        // Server-rendered view props carry `featured_position`.
        router.refresh();
      },
    },
  );
}
