"use client";

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "@/lib/api/queries";
import { showUndoToast } from "@/lib/ui/undo-toast";

import { recreateCollection, type CollectionSnapshot } from "./recreate";

/**
 * Returns `showUndo(snapshot)` — pops a "Collection deleted" toast carrying
 * an **Undo** action (the shared [`showUndoToast`]). Undo replays the
 * snapshot through [`recreateCollection`] and refreshes the collection +
 * saved-view caches so the restored collection reappears.
 *
 * The delete call sites own the snapshot (taken *before* the delete) and
 * the actual delete mutation; this hook only owns the toast + restore, so
 * it can be shared between the detail page and the views-index card.
 */
export function useCollectionDeleteUndo() {
  const qc = useQueryClient();
  return React.useCallback(
    (snap: CollectionSnapshot) => {
      showUndoToast({
        message: `Collection "${snap.name}" deleted`,
        restoredMessage: `Restored "${snap.name}"`,
        failedMessage: "Couldn't restore the collection",
        onUndo: async () => {
          const created = await recreateCollection(snap);
          if (!created) throw new Error("recreate returned null");
          // Both caches feed the surfaces a restored collection shows up
          // on: the collections list/grid and the sidebar (which reads
          // `/me/saved-views`).
          qc.invalidateQueries({ queryKey: queryKeys.collections });
          qc.invalidateQueries({ queryKey: ["saved-views"] });
        },
      });
    },
    [qc],
  );
}
