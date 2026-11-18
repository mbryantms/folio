"use client";

import { BookmarkPlus } from "lucide-react";
import { toast } from "sonner";

import type { SelectionAction } from "@/components/library/SelectionToolbar";
import {
  BULK_ADD_MAX_MEMBERS,
  useBulkAddToWantToRead,
} from "@/lib/api/mutations";
import { useCollections } from "@/lib/api/queries";
import { TOAST } from "@/lib/api/toast-strings";

const WANT_TO_READ_KEY = "want_to_read";

/**
 * The multi-select "Add to Want to Read" — one tap instead of opening the
 * collection picker. Available to every user (it's their own list). Resolves
 * the per-user system collection from the cached collections list, like the
 * cover menu's single-item action (`useCoverMenuCollectionActions`).
 */
export function useWantToReadSelectionAction(
  targets: { entry_kind: "issue" | "series"; ref_id: string }[],
  opts: { onDone?: () => void; disabled?: boolean } = {},
): { action: SelectionAction; isPending: boolean } {
  const collections = useCollections();
  const wtrId =
    collections.data?.find((c) => c.system_key === WANT_TO_READ_KEY)?.id ?? "";
  const add = useBulkAddToWantToRead(wtrId);
  return {
    isPending: add.isPending,
    action: {
      id: "add-to-want-to-read",
      label: "Add to Want to Read",
      icon: BookmarkPlus,
      onClick: () => {
        if (!wtrId) {
          toast.error(TOAST.WTR_NOT_READY);
          return;
        }
        add.mutate({ members: targets }, { onSuccess: () => opts.onDone?.() });
      },
      disabled:
        opts.disabled ||
        add.isPending ||
        targets.length === 0 ||
        targets.length > BULK_ADD_MAX_MEMBERS,
    },
  };
}
