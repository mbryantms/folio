"use client";

import { useCallback, useMemo } from "react";

import {
  useCreateMarker,
  useDeleteMarkerById,
  useRestoreMarkers,
} from "@/lib/api/mutations/markers";
import { useIssueMarkers } from "@/lib/api/queries";
import { showUndoToast } from "@/lib/ui/undo-toast";
import type { MarkerKind, MarkerView } from "@/lib/api/types";

/**
 * Toggle a page-level marker of one `kind` (bookmark / favorite) at a
 * given page. The find-existing → create-or-delete-with-Undo logic used
 * to be copy-pasted in two places — the reader's `b`/`s` keybinds
 * (Reader.tsx) and the chrome's icon buttons (ReaderChrome.tsx), which
 * drifted (different delete-hook shapes, slightly different toast copy).
 * This is the single source of truth; callers supply the removal toast's
 * wording, and creating toasts through `useCreateMarker`.
 *
 * - `existing` is the matching page-level (region-less) marker, or
 *   undefined — drives the active/icon state.
 * - `toggle(labels)` removes it (Undo toast titled `labels.removed`) or
 *   creates it (the hook's "Bookmarked page N" / "Starred page N" toast;
 *   `silentCreate` keeps the chrome buttons' no-toast-on-create
 *   behaviour).
 */
export function usePageMarkerToggle(
  issueId: string,
  pageIndex: number,
  kind: MarkerKind,
  opts?: { silentCreate?: boolean },
): {
  existing: MarkerView | undefined;
  toggle: (labels: { removed: string }) => void;
} {
  const markers = useIssueMarkers(issueId);
  const existing = useMemo(
    () =>
      (markers.data?.items ?? []).find(
        (m) => m.kind === kind && m.page_index === pageIndex && !m.region,
      ),
    [markers.data, pageIndex, kind],
  );
  const create = useCreateMarker({ silent: opts?.silentCreate });
  const restore = useRestoreMarkers();
  // Id arrives at mutate() time, so the hook is never bound to "" and
  // doesn't re-derive on every page turn. `silent` so the caller's
  // labelled Undo toast is the only success signal.
  const del = useDeleteMarkerById(issueId, { silent: true });

  const toggle = useCallback(
    (labels: { removed: string }) => {
      if (existing) {
        const snapshot = existing;
        del.mutate(snapshot.id, {
          onSuccess: () =>
            showUndoToast({
              message: labels.removed,
              onUndo: () => restore.mutateAsync([snapshot]),
            }),
        });
        return;
      }
      create.mutate({ issue_id: issueId, page_index: pageIndex, kind });
    },
    [existing, del, create, restore, issueId, pageIndex, kind],
  );

  return { existing, toggle };
}
