"use client";

import * as React from "react";

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
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";

/** Threshold at or above which the catalog-vs-just-read prompt is
 *  shown. Below it, bulk-marks proceed directly without asking — at
 *  small counts the "I just finished a binge" case dominates and the
 *  prompt is friction. Picked to match the recommendation in the
 *  bulk-mark / reading-log scoping discussion (2026-05-23). */
export const BULK_BACKFILL_PROMPT_THRESHOLD = 10;

/** Confirmation dialog shown before a bulk mark-as-read. The user picks
 *  whether the operation should count toward today's reading activity
 *  (reading log feed, daily heatmap accents, Just Finished saved
 *  view) — defaulted to "catalog update" (backfill = true) since
 *  recording many issues at once is overwhelmingly a "I read these
 *  previously" pattern rather than a binge.
 *
 *  Only used when `count >= BULK_BACKFILL_PROMPT_THRESHOLD`; smaller
 *  bulk-marks call the mutation directly.
 *
 *  Controlled via `open`/`onOpenChange`. The parent calls `onConfirm`
 *  with the chosen `backfill` flag, which it then threads into the
 *  bulk-mark mutation. */
export function BulkMarkReadDialog({
  open,
  onOpenChange,
  count,
  onConfirm,
  isPending = false,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  count: number;
  onConfirm: (backfill: boolean) => void;
  isPending?: boolean;
}) {
  const [backfill, setBackfill] = React.useState(true);
  // Reset to the default whenever the dialog re-opens so the user's
  // last choice doesn't quietly carry over to the next batch.
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    if (open) setBackfill(true);
  }, [open]);

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Mark {count.toLocaleString()} issues as read?
          </AlertDialogTitle>
          <AlertDialogDescription>
            Choose whether this counts as reading activity for today, or
            should just update your collection silently.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <Label
          htmlFor="bulk-mark-backfill"
          className="border-border bg-muted/30 flex cursor-pointer items-start gap-3 rounded-md border p-3 text-sm font-normal"
        >
          <Checkbox
            id="bulk-mark-backfill"
            checked={backfill}
            onCheckedChange={(v) => setBackfill(v === true)}
            className="mt-0.5"
            disabled={isPending}
          />
          <span>
            <span className="font-medium">Updating my collection</span>
            <span className="text-muted-foreground block text-xs">
              Don&apos;t add to my reading log or count toward
              today&apos;s reading activity. Recommended when recording
              issues read previously.
            </span>
          </span>
        </Label>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isPending}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={() => onConfirm(backfill)}
            disabled={isPending}
          >
            Mark read
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
