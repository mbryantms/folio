"use client";

import {
  BookmarkPlus,
  CheckCircle2,
  Circle,
  Folder,
  Image as ImageIcon,
  Images,
  Loader2,
  Pencil,
  RefreshCw,
  RotateCcw,
  Settings2,
} from "lucide-react";
import { useRouter } from "next/navigation";
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
  useGenerateSeriesPageMap,
  useRegenerateSeriesCover,
  useRemoveCollectionEntry,
  useTriggerSeriesScan,
  useUpsertSeriesProgress,
} from "@/lib/api/mutations";
import { useCollections, useMe } from "@/lib/api/queries";
import { TOAST } from "@/lib/api/toast-strings";

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
  firstIssueId,
  onEdit,
  onForceRecreatePageMap,
}: {
  seriesId: string;
  seriesSlug: string;
  seriesName: string;
  libraryId: string;
  /** Lowest-sorted active issue id for the "Read from beginning" item. */
  firstIssueId: string | null;
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

  const markAllRead = () =>
    progress.mutate({ finished: true }, { onSuccess: () => router.refresh() });
  const markAllUnread = () =>
    progress.mutate({ finished: false }, { onSuccess: () => router.refresh() });
  const triggerScan = () =>
    scan.mutate(undefined, { onSuccess: () => router.refresh() });
  const readFromStart = () => {
    if (!firstIssueId) return;
    // `?from=start` makes the reader page bypass saved progress and open at
    // page 0, even when the user is mid-way through a different issue.
    router.push(`/read/${firstIssueId}?from=start`);
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
        <Button variant="outline" size="sm" disabled={busy}>
          {busy ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Settings2 className="mr-2 h-4 w-4" />
          )}
          Actions
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60">
        <DropdownMenuLabel>Reading</DropdownMenuLabel>
        <DropdownMenuGroup>
          {firstIssueId && (
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
    </>
  );
}
