"use client";

import { useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useForceRecreateSeriesPageMap } from "@/lib/api/mutations";
import type { SeriesView } from "@/lib/api/types";

import { SeriesEditDrawer } from "./SeriesEditDrawer";
import { SeriesSettingsMenu } from "./SeriesSettingsMenu";

/**
 * Series page action bar. Owns the edit-drawer state since the dropdown
 * menu auto-closes on item-select (a `<SheetTrigger>` inside the menu
 * would close the menu before the sheet had a chance to mount). Same
 * reason for hosting the page-map force-recreate AlertDialog out here:
 * the menu invokes a callback to open it after closing itself. Admin
 * gating still lives inside the menu — non-admins never see the trigger.
 */
export function SeriesActions({
  series,
  libraryId,
  firstIssueId,
}: {
  series: SeriesView;
  libraryId: string;
  /** Series-scope "Read from beginning" target — typically the lowest-
   *  sorted active issue. `null` when the series has no active issues
   *  (in which case the menu suppresses the item). */
  firstIssueId: string | null;
}) {
  const [editOpen, setEditOpen] = useState(false);
  const [confirmForceRecreate, setConfirmForceRecreate] = useState(false);
  const forceRecreatePageMap = useForceRecreateSeriesPageMap(
    series.slug,
    libraryId,
  );

  return (
    <>
      <SeriesSettingsMenu
        seriesSlug={series.slug}
        libraryId={libraryId}
        firstIssueId={firstIssueId}
        onEdit={() => setEditOpen(true)}
        onForceRecreatePageMap={() => setConfirmForceRecreate(true)}
      />
      <SeriesEditDrawer
        series={series}
        open={editOpen}
        onOpenChange={setEditOpen}
      />
      <AlertDialog
        open={confirmForceRecreate}
        onOpenChange={setConfirmForceRecreate}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rebuild all page thumbnails?</AlertDialogTitle>
            <AlertDialogDescription>
              Every per-page strip thumbnail for this series is deleted from
              disk and re-encoded from the source archives. Cover thumbnails are
              preserved. Use this when the existing strips are stale or
              corrupted; otherwise prefer &ldquo;Fill missing page
              thumbnails&rdquo;, which only encodes pages that aren&apos;t
              already on disk.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => forceRecreatePageMap.mutate()}
              disabled={forceRecreatePageMap.isPending}
            >
              Rebuild all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
