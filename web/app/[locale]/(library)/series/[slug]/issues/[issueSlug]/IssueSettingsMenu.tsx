"use client";

import {
  Bookmark,
  CheckCircle2,
  Circle,
  Download,
  EyeOff,
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
  useGenerateIssuePageMap,
  useRegenerateIssueCover,
  useScanIssue,
  useUpsertIssueProgress,
} from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";
import { readerUrl } from "@/lib/urls";
import type { IssueDetailView } from "@/lib/api/types";
import type { ReadState } from "@/lib/reading-state";

/**
 * Consolidated actions menu for the issue page. Groups read-state +
 * library + admin actions under one trigger so the page header stays
 * compact, and stubs out the "Coming soon" affordances (favorite,
 * bookmark, reading list, collection, download) so users can see what's
 * planned without clicking through to a 404.
 *
 * The "Edit" action is rendered by the dialog wrapper, not here — the
 * dialog's `<DialogTrigger asChild>` wants its own button (a menu item
 * inside the menu would close the menu immediately on click).
 */
export function IssueSettingsMenu({
  issue,
  readState,
  onEdit,
  onForceRecreatePageMap,
}: {
  issue: IssueDetailView;
  readState: ReadState;
  /** Called when the user picks "Edit issue" — the parent owns the
   *  edit-dialog state because the menu auto-closes on item select. */
  onEdit?: () => void;
  /** Called when the user picks "Force recreate page thumbnails" — the
   *  parent owns the AlertDialog state for the same reason as `onEdit`. */
  onForceRecreatePageMap?: () => void;
}) {
  const me = useMe();
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
    router.push(`${readerUrl(issue)}?from=start`);
  };
  const readIncognito = () => {
    // `?incognito=1` disables the reading-session tracker and the
    // /progress writes for this read. Server is never told the issue was
    // opened. Read-from-saved-progress is still respected.
    router.push(`${readerUrl(issue)}?incognito=1`);
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
          <DropdownMenuItem disabled>
            <Bookmark className="mr-2 h-4 w-4" />
            Bookmark
            <SoonBadge />
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
            Favorite
            <SoonBadge />
          </DropdownMenuItem>
          <DropdownMenuItem disabled>
            <Download className="mr-2 h-4 w-4" />
            Download
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
