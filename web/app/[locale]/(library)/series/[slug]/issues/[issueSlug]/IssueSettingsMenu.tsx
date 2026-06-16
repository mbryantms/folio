"use client";

import {
  Bookmark,
  BookmarkPlus,
  CheckCircle2,
  Circle,
  Download,
  EyeOff,
  Folder,
  History,
  Image as ImageIcon,
  Images,
  Layers,
  Link2,
  Loader2,
  Pencil,
  RefreshCw,
  RotateCcw,
  Settings,
  Sparkles,
} from "lucide-react";
import dynamic from "next/dynamic";
import { useRouter, useSearchParams } from "next/navigation";
import { useState } from "react";
import { toast } from "sonner";

import {
  AddToCollectionDialog,
  type AddToCollectionTarget,
} from "@/components/collections/AddToCollectionDialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuPortal,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  useAddCollectionEntry,
  useCreateMarker,
  useDeleteMarker,
  useGenerateIssuePageMap,
  useRegenerateIssueCover,
  useRemoveCollectionEntry,
  useScanIssue,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";
import { useCollections, useIssueMarkers, useMe } from "@/lib/api/queries";
import { TOAST, UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import { markerToCreateReq } from "@/lib/markers/recreate";
import { issueUrl, readerUrl } from "@/lib/urls";
import { useShareLink } from "@/lib/ui/use-share-link";
import type { IssueDetailView } from "@/lib/api/types";
import type { ReadState } from "@/lib/reading-state";

// Heavy match dialog (~1.2k lines + provider-compare UI) — lazy so it stays
// out of the issue-page initial bundle; the chunk loads on first open (G6).
const MetadataMatchDialog = dynamic(
  () =>
    import("@/components/library/MetadataMatchDialog").then(
      (m) => m.MetadataMatchDialog,
    ),
  { ssr: false },
);

const WANT_TO_READ_KEY = "want_to_read";

/**
 * Consolidated actions menu for the issue page. Groups read-state +
 * library + admin actions under one trigger so the page header stays
 * compact:
 *
 *   - **Reading:** read-from-start, incognito, mark read/unread,
 *     bookmark toggle (a page-0 marker).
 *   - **Library:** add to Want to Read, add to collection…, download
 *     (OPDS file route).
 *   - **Admin** (admin role only): rescan issue, thumbnail + page-map
 *     regeneration, edit issue metadata.
 *
 * The "Edit" action is rendered by the dialog wrapper, not here — the
 * dialog's `<DialogTrigger asChild>` wants its own button (a menu item
 * inside the menu would close the menu immediately on click).
 */
export function IssueSettingsMenu({
  issue,
  readState,
  cblSavedViewId,
  onEdit,
  onForceRecreatePageMap,
  onEditArchive,
  onRestoreArchive,
}: {
  issue: IssueDetailView;
  readState: ReadState;
  /** Saved-view id of the CBL the user is reading through (when the
   *  issue page was arrived at via `?cbl=`). Forwarded onto the menu's
   *  read-shortcut URLs so the reader's next-up resolver keeps picking
   *  list entries after a "Read from beginning" or incognito click. */
  cblSavedViewId?: string | null;
  /** Called when the user picks "Edit issue" — the parent owns the
   *  edit-dialog state because the menu auto-closes on item select. */
  onEdit?: () => void;
  /** Called when the user picks "Force recreate page thumbnails" — the
   *  parent owns the AlertDialog state for the same reason as `onEdit`. */
  onForceRecreatePageMap?: () => void;
  /** Called when the user picks "Edit archive…" — opens the page editor.
   *  Only rendered when the parent library has writeback enabled. */
  onEditArchive?: () => void;
  /** Called when the user picks "Restore from backup" — parent owns the
   *  confirm dialog. */
  onRestoreArchive?: () => void;
}) {
  const me = useMe();
  const share = useShareLink();
  const router = useRouter();
  const isAdmin = me.data?.role === "admin";

  const progress = useUpsertIssueProgress();
  const scan = useScanIssue(issue.series_slug, issue.slug);
  const regenerateCover = useRegenerateIssueCover(
    issue.series_slug,
    issue.slug,
    issue.library_id,
  );
  const generatePageMap = useGenerateIssuePageMap(
    issue.series_slug,
    issue.slug,
    issue.library_id,
  );

  // Bookmark = page-0 marker with kind='bookmark'. If one already exists,
  // the menu item toggles it off; otherwise it creates one.
  const issueMarkers = useIssueMarkers(issue.id);
  const existingBookmark = issueMarkers.data?.items.find(
    (m) => m.kind === "bookmark" && m.page_index === 0,
  );
  const createMarker = useCreateMarker();
  const deleteMarker = useDeleteMarker(existingBookmark?.id ?? "", issue.id, {
    silent: true,
  });
  const toggleBookmark = () => {
    if (existingBookmark) {
      const snapshot = existingBookmark;
      deleteMarker.mutate(undefined, {
        onSuccess: () =>
          toast.success("Bookmark removed", {
            duration: UNDO_TOAST_DURATION_MS,
            action: {
              label: "Undo",
              onClick: () => createMarker.mutate(markerToCreateReq(snapshot)),
            },
          }),
      });
    } else {
      createMarker.mutate(
        { issue_id: issue.id, page_index: 0, kind: "bookmark" },
        {
          onSuccess: () => toast.success("Bookmarked"),
        },
      );
    }
  };

  // Want to Read is the per-user auto-seeded collection.
  const collections = useCollections();
  const wantToRead = collections.data?.find(
    (c) => c.system_key === WANT_TO_READ_KEY,
  );
  const wtrId = wantToRead?.id ?? "";
  const addToWtr = useAddCollectionEntry(wtrId);
  const removeFromWtr = useRemoveCollectionEntry(wtrId);
  const [collectionDialogOpen, setCollectionDialogOpen] = useState(false);
  // Deep-link: a "Needs metadata" chip (e.g. the CollectionTab detail link)
  // navigates here with ?match=1 to pop the match dialog straight open —
  // "chips link to the fix" (B4). Seed open-state from the param on mount.
  const searchParams = useSearchParams();
  const [metadataDialogOpen, setMetadataDialogOpen] = useState(
    () => searchParams.get("match") === "1",
  );
  // Mount the lazy match dialog on first open and keep it mounted so its
  // open/close animation still runs on later toggles (G6).
  const [metadataMounted, setMetadataMounted] = useState(false);
  if (metadataDialogOpen && !metadataMounted) setMetadataMounted(true);

  // Toggling the dialog also strips ?match from the address bar (via
  // replaceState, no RSC round-trip — same idiom as IssuesPanel) so a
  // refresh or back-nav doesn't re-pop it after a manual close.
  const handleMetadataOpenChange = (next: boolean) => {
    setMetadataDialogOpen(next);
    if (!next && typeof window !== "undefined") {
      const url = new URL(window.location.href);
      if (url.searchParams.has("match")) {
        url.searchParams.delete("match");
        window.history.replaceState({}, "", url.toString());
      }
    }
  };

  const issueLabel = issue.title ?? `Issue ${issue.number ?? ""}`.trim();

  const addToReadingList = () => {
    if (!wtrId) {
      toast.error(TOAST.WTR_NOT_READY);
      return;
    }
    addToWtr.mutate(
      { entry_kind: "issue", ref_id: issue.id },
      {
        onSuccess: (entry) => {
          if (!entry) {
            toast.success(`Added "${issueLabel}" to Want to Read`);
            return;
          }
          toast.success(`Added "${issueLabel}" to Want to Read`, {
            duration: UNDO_TOAST_DURATION_MS,
            action: {
              label: "Undo",
              onClick: () => removeFromWtr.mutate({ entryId: entry.id }, {}),
            },
          });
        },
      },
    );
  };

  const collectionTarget: AddToCollectionTarget = {
    entry_kind: "issue",
    ref_id: issue.id,
    label: issueLabel,
  };

  const finishedPage = Math.max(0, (issue.page_count ?? 1) - 1);
  // "Read from beginning" only makes sense when the issue is readable.
  // For removed / encrypted / malformed issues we hide it.
  const canRead = issue.state === "active";
  // Item is dynamic per the user's request: only surface when there's
  // existing progress to reset (otherwise the primary "Read" button
  // already starts at page 0).
  const showReadFromStart = canRead && readState !== "unread";

  const markRead = () =>
    progress.mutate(
      { issue_id: issue.id, page: finishedPage, finished: true },
      {
        onSuccess: () => {
          router.refresh();
        },
      },
    );
  const markUnread = () =>
    progress.mutate(
      { issue_id: issue.id, page: 0, finished: false },
      {
        onSuccess: () => {
          router.refresh();
        },
      },
    );
  const triggerScan = () =>
    scan.mutate(undefined, { onSuccess: () => router.refresh() });
  const readFromStart = () => {
    // The reader page reads `?from=start` and skips the saved-progress
    // prefetch, so passing the param avoids a write-then-navigate race.
    const base = readerUrl(issue, { cbl: cblSavedViewId });
    const sep = base.includes("?") ? "&" : "?";
    router.push(`${base}${sep}from=start`);
  };
  const readIncognito = () => {
    // `?incognito=1` disables the reading-session tracker and the
    // /progress writes for this read. Server is never told the issue was
    // opened. Read-from-saved-progress is still respected.
    const base = readerUrl(issue, { cbl: cblSavedViewId });
    const sep = base.includes("?") ? "&" : "?";
    router.push(`${base}${sep}incognito=1`);
  };

  const busy =
    progress.isPending ||
    scan.isPending ||
    regenerateCover.isPending ||
    generatePageMap.isPending ||
    createMarker.isPending ||
    deleteMarker.isPending ||
    addToWtr.isPending;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {/* Compact gear that sits flush to the right of the primary
              `Read` button at every breakpoint — the Read CTA is the focus,
              so actions stay a small icon. Square, matching the Read
              button's height (h-12 mobile / h-10 sm+). */}
          <Button
            variant="outline"
            disabled={busy}
            aria-label="Issue actions"
            className="h-12 w-12 shrink-0 place-items-center p-0 sm:h-10 sm:w-10"
          >
            {busy ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Settings className="h-4 w-4" />
            )}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-60">
          <DropdownMenuLabel>Reading</DropdownMenuLabel>
          <DropdownMenuGroup>
            {showReadFromStart && (
              <DropdownMenuItem onSelect={readFromStart}>
                <RotateCcw className="mr-2 h-4 w-4" />
                Read from beginning
              </DropdownMenuItem>
            )}
            {canRead && (
              <DropdownMenuItem onSelect={readIncognito}>
                <EyeOff className="mr-2 h-4 w-4" />
                Read in incognito
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onSelect={markRead}>
              <CheckCircle2 className="mr-2 h-4 w-4" />
              Mark as read
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={markUnread}>
              <Circle className="mr-2 h-4 w-4" />
              Mark as unread
            </DropdownMenuItem>
            <DropdownMenuItem
              onSelect={toggleBookmark}
              disabled={createMarker.isPending || deleteMarker.isPending}
            >
              <Bookmark className="mr-2 h-4 w-4" />
              {existingBookmark ? "Remove bookmark" : "Bookmark"}
            </DropdownMenuItem>
          </DropdownMenuGroup>

          <DropdownMenuSeparator />
          <DropdownMenuLabel>Library</DropdownMenuLabel>
          <DropdownMenuGroup>
            <DropdownMenuItem
              onSelect={addToReadingList}
              disabled={!wtrId || addToWtr.isPending}
            >
              <BookmarkPlus className="mr-2 h-4 w-4" />
              Add to Want to Read
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setCollectionDialogOpen(true)}>
              <Folder className="mr-2 h-4 w-4" />
              Add to collection…
            </DropdownMenuItem>
            <DropdownMenuItem
              onSelect={() =>
                void share.shareOrCopy(issueUrl(issue), issueLabel)
              }
            >
              <Link2 className="mr-2 h-4 w-4" />
              {share.label}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setMetadataDialogOpen(true)}>
              <Sparkles className="mr-2 h-4 w-4" />
              Fetch metadata…
            </DropdownMenuItem>
            {canRead && (
              <DropdownMenuItem asChild>
                <a href={`/opds/v1/issues/${issue.id}/file`} download>
                  <Download className="mr-2 h-4 w-4" />
                  Download
                </a>
              </DropdownMenuItem>
            )}
          </DropdownMenuGroup>

          {isAdmin && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Admin</DropdownMenuLabel>
              <DropdownMenuGroup>
                <DropdownMenuItem
                  onSelect={triggerScan}
                  disabled={scan.isPending}
                >
                  <RefreshCw className="mr-2 h-4 w-4" />
                  Scan issue
                </DropdownMenuItem>
                <DropdownMenuSub>
                  <DropdownMenuSubTrigger>
                    <Images className="mr-2 h-4 w-4" />
                    Thumbnails
                  </DropdownMenuSubTrigger>
                  <DropdownMenuPortal>
                    <DropdownMenuSubContent>
                      <DropdownMenuItem
                        onSelect={() => regenerateCover.mutate()}
                        disabled={regenerateCover.isPending}
                      >
                        <ImageIcon className="mr-2 h-4 w-4" />
                        Rebuild cover
                      </DropdownMenuItem>
                      <DropdownMenuItem
                        onSelect={() => generatePageMap.mutate()}
                        disabled={generatePageMap.isPending}
                      >
                        <Images className="mr-2 h-4 w-4" />
                        Fill missing page thumbnails
                      </DropdownMenuItem>
                      {onForceRecreatePageMap && (
                        <DropdownMenuItem
                          onSelect={onForceRecreatePageMap}
                          className="text-destructive focus:text-destructive"
                        >
                          <Images className="mr-2 h-4 w-4" />
                          Rebuild all page thumbnails
                        </DropdownMenuItem>
                      )}
                    </DropdownMenuSubContent>
                  </DropdownMenuPortal>
                </DropdownMenuSub>
                {onEdit && (
                  <DropdownMenuItem onSelect={onEdit}>
                    <Pencil className="mr-2 h-4 w-4" />
                    Edit issue
                  </DropdownMenuItem>
                )}
                {issue.allow_archive_writeback && onEditArchive && canRead && (
                  <DropdownMenuItem onSelect={onEditArchive}>
                    <Layers className="mr-2 h-4 w-4" />
                    Edit archive…
                  </DropdownMenuItem>
                )}
                {issue.allow_archive_writeback &&
                  onRestoreArchive &&
                  issue.last_rewrite_at && (
                    <DropdownMenuItem
                      onSelect={onRestoreArchive}
                      className="text-destructive focus:text-destructive"
                    >
                      <History className="mr-2 h-4 w-4" />
                      Restore from backup…
                    </DropdownMenuItem>
                  )}
              </DropdownMenuGroup>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <AddToCollectionDialog
        open={collectionDialogOpen}
        onOpenChange={setCollectionDialogOpen}
        target={collectionTarget}
      />
      {metadataMounted && (
        <MetadataMatchDialog
          open={metadataDialogOpen}
          onOpenChange={handleMetadataOpenChange}
          scope={{
            kind: "issue",
            seriesSlug: issue.series_slug,
            issueSlug: issue.slug,
            libraryId: issue.library_id,
          }}
        />
      )}
    </>
  );
}
