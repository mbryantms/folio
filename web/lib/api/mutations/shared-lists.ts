/**
 * Shared reading lists: an admin shares one of their own reading lists
 * with every user. Re-exported from `index.ts`.
 */
import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";

import { useApiMutation } from "./_core";
import { apiPath } from "../path";

/** `POST /admin/saved-views/{id}/share` — the admin's own reading list
 *  becomes a shared list every user can read (admins manage it). */
export function useShareReadingList(viewId: string) {
  const qc = useQueryClient();
  const router = useRouter();
  return useApiMutation<unknown, void>(
    () => ({
      path: apiPath`/admin/saved-views/${viewId}/share`,
      method: "POST",
    }),
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
