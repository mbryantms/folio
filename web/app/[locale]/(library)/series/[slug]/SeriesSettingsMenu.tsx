"use client";

import {
  BookmarkPlus,
  CheckCircle2,
  Circle,
  EyeOff,
  Folder,
  Image as ImageIcon,
  Images,
  Loader2,
  Pencil,
  RefreshCw,
  RotateCcw,
  Settings,
  Sparkles,
} from "lucide-react";
import dynamic from "next/dynamic";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { toast } from "sonner";

import {
  AddToCollectionDialog,
  type AddToCollectionTarget,
} from "@/components/collections/AddToCollectionDialog";
import { BulkMarkReadDialog } from "@/components/library/BulkMarkReadDialog";
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
  useCreateSeriesBatch,
  useGenerateSeriesPageMap,
  useRegenerateSeriesCover,
  useRemoveCollectionEntry,
  useTriggerSeriesScan,
  useUpsertSeriesProgress,
} from "@/lib/api/mutations";
import { useCollections, useMe } from "@/lib/api/queries";
import { TOAST, UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import type { IssueSummaryView } from "@/lib/api/types";
import { readerUrl } from "@/lib/urls";

// Heavy match dialog (~1.2k lines + provider-compare UI) — lazy so it stays
// out of the series-page initial bundle; the chunk loads on first open (G6).
const MetadataMatchDialog = dynamic(
  () =>
    import("@/components/library/MetadataMatchDialog").then(
      (m) => m.MetadataMatchDialog,
    ),
  { ssr: false },
);

const WANT_TO_READ_KEY = "want_to_read";

/**
 * Companion to `IssueSettingsMenu` for the series page. Same trigger /
 * grouping shape — keeps the two pages predictable side-by-side. Bulk
 * mark-all routes through `POST /series/{id}/progress`, so a 200-issue
 * series is one round trip, not 200.
 */
export function SeriesSettingsMenu({
  seriesId,
  seriesSlug,
  seriesName,
  libraryId,
  firstIssue,
  readIncognitoHref,
  onEdit,
  onForceRecreatePageMap,
}: {
  seriesId: string;
  seriesSlug: string;
  seriesName: string;
  libraryId: string;
  /** Lowest-sorted active issue for the "Read from beginning" item. */
  firstIssue: Pick<IssueSummaryView, "slug" | "series_slug"> | null;
  /** Reader URL for the up-next unread issue with incognito enabled —
   *  same target as the primary Read button, `+incognito=1`. `null` when
   *  the series has nothing to resume. */
  readIncognitoHref: string | null;
  /** Called when the user picks "Edit series" — the parent owns the
   *  edit-drawer state because the menu auto-closes on item select. */
  onEdit?: () => void;
  /** Called when the user picks "Force recreate page thumbnails" — the
   *  parent owns the AlertDialog state for the same reason as `onEdit`. */
  onForceRecreatePageMap?: () => void;
}) {
  const me = useMe();
  const router = useRouter();
  const isAdmin = me.data?.role === "admin";

  const progress = useUpsertSeriesProgress(seriesSlug);
  const scan = useTriggerSeriesScan(seriesSlug, libraryId);
  const regenerateCover = useRegenerateSeriesCover(seriesSlug, libraryId);
  const generatePageMap = useGenerateSeriesPageMap(seriesSlug, libraryId);
  const createBatch = useCreateSeriesBatch(seriesSlug);

  const fetchAllMetadata = (scope: "all" | "incomplete" = "all") => {
    createBatch.mutate(
      { scope },
      {
        onSuccess: (resp) => {
          if (!resp) return;
          if (resp.items_total === 0) {
            toast.info(
              scope === "incomplete"
                ? "Every issue already has complete metadata"
                : "No issues to search",
            );
            return;
          }
          toast.success(`Searching ${resp.items_total} issues for metadata`, {
            action: {
              label: "Review",
              onClick: () =>
                router.push(
                  `/admin/metadata?tab=review&batch=${resp.batch_id}`,
                ),
            },
          });
        },
      },
    );
  };

  // Want to Read is the per-user auto-seeded collection (system_key='want_to_read').
  // The sidebar fetch of /me/collections seeds it on first load; by the time the
  // user opens the actions menu the row is almost always already present.
  const collections = useCollections();
  const wantToRead = collections.data?.find(
    (c) => c.system_key === WANT_TO_READ_KEY,
  );
  const wtrId = wantToRead?.id ?? "";
  const addToWtr = useAddCollectionEntry(wtrId);
  const removeFromWtr = useRemoveCollectionEntry(wtrId);
  const [collectionDialogOpen, setCollectionDialogOpen] = useState(false);
  const [metadataDialogOpen, setMetadataDialogOpen] = useState(false);
  // Mount the lazy match dialog on first open and keep it mounted so its
  // open/close animation still runs on later toggles (G6).
  const [metadataMounted, setMetadataMounted] = useState(false);
  if (metadataDialogOpen && !metadataMounted) setMetadataMounted(true);

  const addToReadingList = () => {
    if (!wtrId) {
      toast.error(TOAST.WTR_NOT_READY);
      return;
    }
    addToWtr.mutate(
      { entry_kind: "series", ref_id: seriesId },
      {
        onSuccess: (entry) => {
          if (!entry) {
            toast.success(`Added "${seriesName}" to Want to Read`);
            return;
          }
          toast.success(`Added "${seriesName}" to Want to Read`, {
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
    entry_kind: "series",
    ref_id: seriesId,
    label: seriesName,
  };

  // "Mark all as read" on a whole series is almost always cataloging
  // ("I read this years ago"), not active reading — prompt before
  // writing so the user can choose whether it should count toward
  // reading activity. Unread + per-issue paths skip the prompt.
  const [markAllReadOpen, setMarkAllReadOpen] = useState(false);
  const markAllRead = () => setMarkAllReadOpen(true);
  const submitMarkAllRead = (backfill: boolean) =>
    progress.mutate(
      { finished: true, backfill },
      {
        onSuccess: () => {
          setMarkAllReadOpen(false);
          router.refresh();
        },
      },
    );
  const markAllUnread = () =>
    progress.mutate({ finished: false }, { onSuccess: () => router.refresh() });
  const triggerScan = () =>
    scan.mutate(undefined, { onSuccess: () => router.refresh() });
  const readFromStart = () => {
    if (!firstIssue) return;
    // `?from=start` makes the reader page bypass saved progress and open at
    // page 0, even when the user is mid-way through a different issue.
    router.push(`${readerUrl(firstIssue)}?from=start`);
  };
  // Same target as the primary Read button (the up-next unread issue), but
  // with incognito on so the read isn't tracked.
  const readIncognito = () => {
    if (readIncognitoHref) router.push(readIncognitoHref);
  };

  const busy =
    progress.isPending ||
    scan.isPending ||
    regenerateCover.isPending ||
    generatePageMap.isPending ||
    addToWtr.isPending;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {/* Compact gear that sits flush to the right of the primary
              `Read` button — the Read CTA is the focus, so actions stay a
              small icon (mirrors `IssueSettingsMenu`). Square, matching the
              Read button's height at each breakpoint. */}
          <Button
            variant="outline"
            disabled={busy}
            aria-label="Series actions"
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
            {readIncognitoHref && (
              <DropdownMenuItem onSelect={readIncognito}>
                <EyeOff className="mr-2 h-4 w-4" />
                Read incognito
              </DropdownMenuItem>
            )}
            {firstIssue && (
              <DropdownMenuItem onSelect={readFromStart}>
                <RotateCcw className="mr-2 h-4 w-4" />
                Read from beginning
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onSelect={markAllRead}>
              <CheckCircle2 className="mr-2 h-4 w-4" />
              Mark all as read
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={markAllUnread}>
              <Circle className="mr-2 h-4 w-4" />
              Mark all as unread
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
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <Sparkles className="mr-2 h-4 w-4" />
                Fetch metadata
              </DropdownMenuSubTrigger>
              <DropdownMenuPortal>
                <DropdownMenuSubContent>
                  <DropdownMenuItem
                    onSelect={() => setMetadataDialogOpen(true)}
                  >
                    <Sparkles className="mr-2 h-4 w-4" />
                    Match this series…
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onSelect={() => fetchAllMetadata("all")}
                    disabled={createBatch.isPending}
                  >
                    <Sparkles className="mr-2 h-4 w-4" />
                    All issues
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onSelect={() => fetchAllMetadata("incomplete")}
                    disabled={createBatch.isPending}
                  >
                    <Sparkles className="mr-2 h-4 w-4" />
                    Only missing or partial
                  </DropdownMenuItem>
                </DropdownMenuSubContent>
              </DropdownMenuPortal>
            </DropdownMenuSub>
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
                  Scan series
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
                    Edit series
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
      <BulkMarkReadDialog
        open={markAllReadOpen}
        onOpenChange={setMarkAllReadOpen}
        count={0}
        title={`Mark every issue in ${seriesName} as read?`}
        onConfirm={submitMarkAllRead}
        isPending={progress.isPending}
      />
      {metadataMounted && (
        <MetadataMatchDialog
          open={metadataDialogOpen}
          onOpenChange={setMetadataDialogOpen}
          scope={{ kind: "series", seriesSlug, libraryId }}
        />
      )}
    </>
  );
}
