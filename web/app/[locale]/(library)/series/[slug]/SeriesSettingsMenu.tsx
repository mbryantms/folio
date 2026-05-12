"use client";

import {
  CheckCircle2,
  Circle,
  Download,
  Folder,
  Heart,
  Image as ImageIcon,
  Images,
  ListPlus,
  Loader2,
  Pencil,
  RefreshCw,
  RotateCcw,
  Settings2,
} from "lucide-react";
import { useRouter } from "next/navigation";

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
  useGenerateSeriesPageMap,
  useRegenerateSeriesCover,
  useTriggerSeriesScan,
  useUpsertSeriesProgress,
} from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";

/**
 * Companion to `IssueSettingsMenu` for the series page. Same trigger /
 * grouping shape — keeps the two pages predictable side-by-side. Bulk
 * mark-all routes through `POST /series/{id}/progress`, so a 200-issue
 * series is one round trip, not 200.
 */
export function SeriesSettingsMenu({
  seriesSlug,
  libraryId,
  firstIssueId,
  onEdit,
  onForceRecreatePageMap,
}: {
  seriesSlug: string;
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
    generatePageMap.isPending;

  return (
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
          <DropdownMenuItem disabled>
            <ListPlus className="mr-2 h-4 w-4" />
            Add to reading list
            <SoonBadge />
          </DropdownMenuItem>
          <DropdownMenuItem disabled>
            <Folder className="mr-2 h-4 w-4" />
            Add to collection
            <SoonBadge />
          </DropdownMenuItem>
          <DropdownMenuItem disabled>
            <Heart className="mr-2 h-4 w-4" />
            Favorite series
            <SoonBadge />
          </DropdownMenuItem>
          <DropdownMenuItem disabled>
            <Download className="mr-2 h-4 w-4" />
            Download series
            <SoonBadge />
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
  );
}

function SoonBadge() {
  return (
    <span className="border-border text-muted-foreground ml-auto rounded-sm border px-1.5 py-0.5 text-[10px] font-semibold tracking-wider uppercase">
      Soon
    </span>
  );
}
