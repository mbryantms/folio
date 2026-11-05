"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { toast } from "sonner";

import {
  useCreateMarker,
  useDeleteMarker,
  useRestoreMarkers,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";
import { useIssueMarkers, useMe } from "@/lib/api/queries";
import type { IssueDetailView } from "@/lib/api/types";
import { showUndoToast } from "@/lib/ui/undo-toast";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { readerUrl } from "@/lib/urls";

import { ISSUE_SHORTCUTS } from "./issue-shortcuts";

export { ISSUE_SHORTCUTS, type IssueShortcutAction } from "./issue-shortcuts";

interface IssueShortcutsOptions {
  /** Toggle the entire hook on/off — useful when a modal is open and we
   *  don't want bare-letter keys leaking through. Defaults to `true`. */
  enabled?: boolean;
  /** Called when the user presses `e`. Admin-gated by the hook; the
   *  parent owns the actual edit-sheet state. */
  onEdit?: () => void;
}

/**
 * Issue-page keyboard shortcuts (M5), as listed in [`ISSUE_SHORTCUTS`]:
 * r mark read, u mark unread, b toggle bookmark, i read in incognito,
 * e edit issue (admin). Hard-coded (not in the keybind registry) for v1;
 * promote if users start asking for rebinds.
 *
 * Behavior parity with `IssueSettingsMenu`: mark-read / mark-unread go
 * through the optimistic `useUpsertIssueProgress` (the read CTA flips at
 * once; the hook raises the Undo toast), bookmark toggles with an Undo
 * toast (matches the menu's bookmark behavior), incognito navigates to
 * the reader (the page change is the signal), edit defers to the parent
 * `onEdit` so it can pop its sheet (closing the menu would dismiss the
 * sheet otherwise).
 */
export function useIssueShortcuts(
  issue: IssueDetailView,
  opts: IssueShortcutsOptions = {},
): void {
  const enabled = opts.enabled !== false;
  const router = useRouter();
  const me = useMe();
  const isAdmin = me.data?.role === "admin";

  const progress = useUpsertIssueProgress();

  // Bookmark = page-0 marker, kind='bookmark'. Mirrors the
  // toggleBookmark logic in IssueSettingsMenu so muscle memory carries
  // over from the menu to the keyboard.
  const issueMarkers = useIssueMarkers(issue.id);
  const existingBookmark = issueMarkers.data?.items.find(
    (m) => m.kind === "bookmark" && m.page_index === 0,
  );
  const createMarker = useCreateMarker();
  const restoreMarkers = useRestoreMarkers();
  const deleteMarker = useDeleteMarker(existingBookmark?.id ?? "", issue.id, {
    silent: true,
  });

  const finishedPage = Math.max(0, (issue.page_count ?? 1) - 1);
  const canRead = issue.state === "active";

  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      // Bare-key shortcuts only — never fire on chords so we don't
      // collide with future modifier-bound actions or browser defaults.
      if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
      const key = e.key.toLowerCase();
      const hit = ISSUE_SHORTCUTS.find((s) => s.key === key);
      if (!hit) return;
      switch (hit.action) {
        case "markRead": {
          e.preventDefault();
          progress.mutate(
            { issue_id: issue.id, page: finishedPage, finished: true },
            { onSuccess: () => router.refresh() },
          );
          break;
        }
        case "markUnread": {
          e.preventDefault();
          progress.mutate(
            { issue_id: issue.id, page: 0, finished: false },
            { onSuccess: () => router.refresh() },
          );
          break;
        }
        case "toggleBookmark": {
          e.preventDefault();
          if (existingBookmark) {
            const snapshot = existingBookmark;
            deleteMarker.mutate(undefined, {
              onSuccess: () =>
                showUndoToast({
                  message: "Bookmark removed",
                  onUndo: () => restoreMarkers.mutateAsync([snapshot]),
                }),
            });
          } else {
            createMarker.mutate(
              { issue_id: issue.id, page_index: 0, kind: "bookmark" },
              { onSuccess: () => toast.success("Bookmarked") },
            );
          }
          break;
        }
        case "readIncognito": {
          if (!canRead) return;
          e.preventDefault();
          router.push(`${readerUrl(issue)}?incognito=1`);
          break;
        }
        case "editIssue": {
          if (!isAdmin || !opts.onEdit) return;
          e.preventDefault();
          opts.onEdit();
          break;
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    enabled,
    issue,
    finishedPage,
    canRead,
    isAdmin,
    opts,
    progress,
    existingBookmark,
    createMarker,
    restoreMarkers,
    deleteMarker,
    router,
  ]);
}
