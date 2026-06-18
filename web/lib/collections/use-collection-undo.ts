"use client";

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { queryKeys } from "@/lib/api/queries";

import { recreateCollection, type CollectionSnapshot } from "./recreate";

/** Undo toasts linger longer than a default toast — a collection delete is
 *  a heavier action than a marker delete, so give the user more time to
 *  catch it. Matches the marker-undo window. */
const UNDO_TOAST_DURATION_MS = 10_000;

/**
 * Returns `showUndo(snapshot)` — pops a "Collection deleted" toast carrying
 * an **Undo** action. Undo replays the snapshot through
 * [`recreateCollection`] and refreshes the collection + saved-view caches
 * so the restored collection reappears.
 *
 * The delete call sites own the snapshot (taken *before* the delete) and
 * the actual delete mutation; this hook only owns the toast + restore, so
 * it can be shared between the detail page and the views-index card.
 */
export function useCollectionDeleteUndo() {
  const qc = useQueryClient();
  return React.useCallback(
    (snap: CollectionSnapshot) => {
      toast.success(`Collection "${snap.name}" deleted`, {
        duration: UNDO_TOAST_DURATION_MS,
        action: {
          label: "Undo",
          onClick: () => {
            void (async () => {
              try {
                const created = await recreateCollection(snap);
                if (!created) throw new Error("recreate returned null");
                // Both caches feed the surfaces a restored collection shows
                // up on: the collections list/grid and the sidebar (which
                // reads `/me/saved-views`).
                qc.invalidateQueries({ queryKey: queryKeys.collections });
                qc.invalidateQueries({ queryKey: ["saved-views"] });
                toast.success(`Restored "${snap.name}"`);
              } catch {
                toast.error("Couldn't restore the collection");
              }
            })();
          },
        },
      });
    },
    [qc],
  );
}
