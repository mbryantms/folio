"use client";

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { useDeletePage } from "@/lib/api/mutations";
import { queryKeys } from "@/lib/api/queries";
import type { PageView } from "@/lib/api/types";
import { showUndoToast } from "@/lib/ui/undo-toast";

import { recreatePage, snapshotPage } from "./recreate";

/**
 * Delete a custom page immediately and offer Undo ("Undo instead of
 * confirm"). The snapshot — name, description, sidebar visibility, rails
 * in order, slot among the pages — is taken before the delete; Undo
 * replays it through [`recreatePage`].
 *
 * `leaveRoute`: the caller is *on* the page (its toolbar menu), so the
 * delete bounces home and Undo navigates to the restored page. Settings
 * → Pages passes `false` and stays put.
 */
export function useDeletePageWithUndo(
  pageId: string,
  { leaveRoute }: { leaveRoute: boolean },
) {
  const qc = useQueryClient();
  const router = useRouter();
  const del = useDeletePage(pageId, { silent: true, leaveRoute });
  const [snapshotting, setSnapshotting] = React.useState(false);

  const run = React.useCallback(async () => {
    setSnapshotting(true);
    let snap;
    try {
      snap = await snapshotPage(
        pageId,
        qc.getQueryData<PageView[]>(queryKeys.mePages),
      );
    } catch {
      toast.error("Couldn't delete the page — try again");
      return;
    } finally {
      setSnapshotting(false);
    }
    del.mutate(undefined, {
      onSuccess: () =>
        showUndoToast({
          message: `Deleted "${snap.name}"`,
          restoredMessage: `Restored "${snap.name}"`,
          failedMessage: "Couldn't restore the page",
          onUndo: async () => {
            const page = await recreatePage(snap);
            qc.invalidateQueries({ queryKey: queryKeys.mePages });
            qc.invalidateQueries({ queryKey: queryKeys.sidebarLayout });
            qc.invalidateQueries({ queryKey: ["saved-views"] });
            if (leaveRoute) router.push(`/pages/${page.slug}`);
            // The left-nav sidebar is server-rendered in the layout.
            router.refresh();
          },
        }),
    });
  }, [pageId, qc, del, leaveRoute, router]);

  return { run, isPending: snapshotting || del.isPending };
}
