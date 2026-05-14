"use client";

import * as React from "react";
import { toast } from "sonner";

import type { CoverMenuAction } from "@/components/CoverMenuButton";
import { useCollections } from "@/lib/api/queries";
import {
  useAddCollectionEntry,
  useRemoveCollectionEntry,
} from "@/lib/api/mutations";
import { TOAST } from "@/lib/api/toast-strings";
import type { CollectionEntryKind } from "@/lib/api/types";

import {
  AddToCollectionDialog,
  type AddToCollectionTarget,
} from "./AddToCollectionDialog";

const WANT_TO_READ_KEY = "want_to_read";

/** Shared hook that injects "Add to Want to Read" + "Add to Collection…"
 *  into a `<CoverMenuButton>`'s action list. Returns the new actions plus
 *  the dialog element the consumer must render at the card root.
 *
 *  Want to Read is dispatched via `useAddCollectionEntry` against the
 *  per-user system collection id (auto-seeded on first
 *  GET /me/saved-views — the sidebar drives that call so the row is
 *  always present by the time a card is rendered). The full "Add to
 *  Collection…" flow opens `<AddToCollectionDialog>`. */
export function useCoverMenuCollectionActions(opts: {
  entry_kind: CollectionEntryKind;
  ref_id: string;
  /** Display label used in toasts ("Added to Want to Read: <label>"). */
  label: string;
}): { actions: CoverMenuAction[]; dialog: React.ReactNode } {
  const { entry_kind, ref_id, label } = opts;
  const collections = useCollections();
  const wantToRead = collections.data?.find(
    (c) => c.system_key === WANT_TO_READ_KEY,
  );
  const wtrId = wantToRead?.id ?? "";
  // The hook is always called — but `mutate` is no-op if the id is empty.
  // We block the action below until the WTR row resolves.
  const addToWtr = useAddCollectionEntry(wtrId);
  const removeFromWtr = useRemoveCollectionEntry(wtrId);
  const [dialogOpen, setDialogOpen] = React.useState(false);

  const actions: CoverMenuAction[] = [
    {
      label: "Add to Want to Read",
      disabled: !wtrId || addToWtr.isPending,
      onSelect: () => {
        if (!wtrId) {
          toast.error(TOAST.WTR_NOT_READY);
          return;
        }
        addToWtr.mutate(
          { entry_kind, ref_id },
          {
            // Stash the inserted entry's id on the toast so Undo can
            // delete exactly the row we just created (the partial unique
            // makes re-add idempotent, so without the id Undo could
            // accidentally remove an older row of the same ref). The
            // server returns `CollectionEntryView` on success, but
            // `useApiMutation` widens to `TData | null` — guard so we
            // never offer an Undo we can't fulfill.
            onSuccess: (entry) => {
              if (!entry) {
                toast.success(`Added "${label}" to Want to Read`);
                return;
              }
              toast.success(`Added "${label}" to Want to Read`, {
                action: {
                  label: "Undo",
                  onClick: () =>
                    removeFromWtr.mutate({ entryId: entry.id }, {}),
                },
              });
            },
          },
        );
      },
    },
    {
      label: "Add to Collection…",
      onSelect: () => setDialogOpen(true),
    },
  ];

  const target: AddToCollectionTarget = { entry_kind, ref_id, label };
  const dialog = (
    <AddToCollectionDialog
      open={dialogOpen}
      onOpenChange={setDialogOpen}
      target={target}
    />
  );

  return { actions, dialog };
}
