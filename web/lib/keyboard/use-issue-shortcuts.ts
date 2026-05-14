"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { toast } from "sonner";

import {
  useCreateMarker,
  useDeleteMarker,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";
import { useIssueMarkers, useMe } from "@/lib/api/queries";
import type { IssueDetailView } from "@/lib/api/types";
import { markerToCreateReq } from "@/lib/markers/recreate";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { readerUrl } from "@/lib/urls";

interface IssueShortcutsOptions {
  /** Toggle the entire hook on/off — useful when a modal is open and we
   *  don't want bare-letter keys leaking through. Defaults to `true`. */
  enabled?: boolean;
  /** Called when the user presses `e`. Admin-gated by the hook; the
   *  parent owns the actual edit-sheet state. */
  onEdit?: () => void;
}

/**
 * Issue-page keyboard shortcuts (M5). Hard-coded (not in the keybind
 * registry) for v1; promote if users start asking for rebinds.
 *
 *   r  Mark read           u  Mark unread
 *   b  Toggle bookmark     i  Read in incognito
 *   e  Edit issue (admin)
 *
 * Behavior parity with `IssueSettingsMenu`: mark-read / mark-unread stay
 * silent (state-update is the signal), bookmark toggles with an Undo
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
      switch (e.key.toLowerCase()) {
        case "r": {
          e.preventDefault();
          progress.mutate(
            { issue_id: issue.id, page: finishedPage, finished: true },
            { onSuccess: () => router.refresh() },
          );
          break;
        }
        case "u": {
          e.preventDefault();
          progress.mutate(
            { issue_id: issue.id, page: 0, finished: false },
            { onSuccess: () => router.refresh() },
          );
          break;
        }
        case "b": {
          e.preventDefault();
          if (existingBookmark) {
            const snapshot = existingBookmark;
            deleteMarker.mutate(undefined, {
              onSuccess: () =>
                toast.success("Bookmark removed", {
                  action: {
                    label: "Undo",
                    onClick: () =>
                      createMarker.mutate(markerToCreateReq(snapshot)),
                  },
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
        case "i": {
          if (!canRead) return;
          e.preventDefault();
          router.push(`${readerUrl(issue)}?incognito=1`);
          break;
        }
        case "e": {
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
    deleteMarker,
    router,
  ]);
}
