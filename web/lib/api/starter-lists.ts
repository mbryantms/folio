/**
 * Starter reading lists (onboarding PR C): the shelf the first-run tour
 * ends on, plus the admin editor. Kept out of the `queries` / `mutations`
 * barrels so the reader's first-load bundle never carries it.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { apiMutate, useApiMutation } from "./mutations/_core";
import { jsonFetch, queryKeys } from "./queries";
import type {
  AddStarterListReq,
  CblListView,
  SavedViewView,
  SetStarterListVisibilityReq,
  StarterListListView,
  StarterListView,
} from "./types";
import { apiPath } from "./path";

export type { StarterListView };

export const starterListKeys = {
  mine: ["me", "starter-lists"] as const,
  admin: ["admin", "starter-lists"] as const,
};

/** Poll while any list's first coverage run is still going. */
const pollWhilePending = (data: StarterListListView | undefined) =>
  data?.items.some((i) => i.coverage_status === "pending") ? 3000 : false;

export function useStarterLists({ enabled = true } = {}) {
  return useQuery({
    queryKey: starterListKeys.mine,
    queryFn: () => jsonFetch<StarterListListView>("/me/starter-lists"),
    enabled,
    refetchInterval: (q) => pollWhilePending(q.state.data),
  });
}

export function useAdminStarterLists() {
  return useQuery({
    queryKey: starterListKeys.admin,
    queryFn: () => jsonFetch<StarterListListView>("/admin/starter-lists"),
    refetchInterval: (q) => pollWhilePending(q.state.data),
  });
}

/** One-click import from the tour: the same two calls the import dialog
 *  makes (create the list from the catalog, then save it as a reading-
 *  list view), named after the starter pick. Returns the new view. */
export function useImportStarterList() {
  const qc = useQueryClient();
  return useMutation<SavedViewView, Error, StarterListView>({
    mutationFn: async (item) => {
      const list = await apiMutate<CblListView>({
        path: "/me/cbl-lists",
        method: "POST",
        body: {
          kind: "catalog",
          catalog_source_id: item.catalog_source_id,
          catalog_path: item.catalog_path,
        },
      });
      if (!list) throw new Error("Import returned no list");
      const view = await apiMutate<SavedViewView>({
        path: "/me/saved-views",
        method: "POST",
        body: {
          kind: "cbl",
          name: item.title,
          description: item.reason || null,
          cbl_list_id: list.id,
        },
      });
      if (!view) throw new Error("Saving the view returned nothing");
      return view;
    },
    onSuccess: (_view, item) => {
      toast.success(`Added “${item.title}” to your reading lists`);
      qc.invalidateQueries({ queryKey: starterListKeys.mine });
      qc.invalidateQueries({ queryKey: queryKeys.cblLists });
      qc.invalidateQueries({ queryKey: ["saved-views"] });
    },
    onError: (e, item) => {
      toast.error(
        e instanceof Error
          ? `Couldn't add “${item.title}”: ${e.message}`
          : `Couldn't add “${item.title}”`,
      );
    },
  });
}

function useAdminWrite<TInput>(
  build: (input: TInput) => Parameters<typeof apiMutate>[0],
  successMessage: string,
) {
  const qc = useQueryClient();
  return useApiMutation<StarterListListView, TInput>(build, {
    successMessage,
    onSuccess: (data) => {
      if (data) qc.setQueryData(starterListKeys.admin, data);
      qc.invalidateQueries({ queryKey: starterListKeys.mine });
    },
  });
}

export function useAddStarterList() {
  return useAdminWrite<AddStarterListReq>(
    (body) => ({ path: "/admin/starter-lists", method: "POST", body }),
    "Starter list added",
  );
}

export function useSetStarterListHidden() {
  return useAdminWrite<SetStarterListVisibilityReq>(
    (body) => ({
      path: "/admin/starter-lists/visibility",
      method: "PUT",
      body,
    }),
    "Starter lists updated",
  );
}

export function useRemoveStarterList() {
  return useAdminWrite<string>(
    (id) => ({
      path: apiPath`/admin/starter-lists/${id}`,
      method: "DELETE",
    }),
    "Starter list removed",
  );
}

export function useRefreshStarterCoverage() {
  const qc = useQueryClient();
  return useApiMutation<null, void>(
    () => ({ path: "/admin/starter-lists/refresh-coverage", method: "POST" }),
    {
      successMessage: "Recounting coverage in the background",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: starterListKeys.admin });
      },
    },
  );
}
