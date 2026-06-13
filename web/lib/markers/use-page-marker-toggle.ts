"use client";

import { useCallback, useMemo } from "react";
import { toast } from "sonner";

import { useCreateMarker, useDeleteMarkerById } from "@/lib/api/mutations";
import { useIssueMarkers } from "@/lib/api/queries";
import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import type { MarkerKind, MarkerView } from "@/lib/api/types";
import { markerToCreateReq } from "@/lib/markers/recreate";

/**
 * Toggle a page-level marker of one `kind` (bookmark / favorite) at a
 * given page. The find-existing → create-or-delete-with-Undo logic used
 * to be copy-pasted in two places — the reader's `b`/`s` keybinds
 * (Reader.tsx) and the chrome's icon buttons (ReaderChrome.tsx), which
 * drifted (different delete-hook shapes, slightly different toast copy).
 * This is the single source of truth; callers supply their own toast
 * strings so each surface keeps its exact wording.
 *
 * - `existing` is the matching page-level (region-less) marker, or
 *   undefined — drives the active/icon state.
 * - `toggle(labels)` removes it (Undo toast titled `labels.removed`) or
 *   creates it (success toast `labels.created`, omitted = silent create,
 *   matching the chrome buttons' original no-toast-on-create behaviour).
 */
export function usePageMarkerToggle(
  issueId: string,
  pageIndex: number,
  kind: MarkerKind,
): {
  existing: MarkerView | undefined;
  toggle: (labels: { created?: string; removed: string }) => void;
} {
  const markers = useIssueMarkers(issueId);
  const existing = useMemo(
    () =>
      (markers.data?.items ?? []).find(
        (m) => m.kind === kind && m.page_index === pageIndex && !m.region,
      ),
    [markers.data, pageIndex, kind],
  );
  const create = useCreateMarker();
  // Id arrives at mutate() time, so the hook is never bound to "" and
  // doesn't re-derive on every page turn. `silent` so the caller's
  // labelled Undo toast is the only success signal.
  const del = useDeleteMarkerById(issueId, { silent: true });

  const toggle = useCallback(
    (labels: { created?: string; removed: string }) => {
      if (existing) {
        const snapshot = existing;
        del.mutate(snapshot.id, {
          onSuccess: () =>
            toast.success(labels.removed, {
              duration: UNDO_TOAST_DURATION_MS,
              action: {
                label: "Undo",
                onClick: () => create.mutate(markerToCreateReq(snapshot)),
              },
            }),
        });
        return;
      }
      const created = labels.created;
      create.mutate(
        { issue_id: issueId, page_index: pageIndex, kind },
        created ? { onSuccess: () => toast.success(created) } : undefined,
      );
    },
    [existing, del, create, issueId, pageIndex, kind],
  );

  return { existing, toggle };
}
