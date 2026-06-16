"use client";

import * as React from "react";
import { Download, RotateCcw } from "lucide-react";

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { PageHeader } from "@/components/admin/PageHeader";
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
import { Button } from "@/components/ui/button";
import { useResetLogWidgets } from "@/lib/api/mutations";
import type { LogWidgetView, ReadingStatsRange } from "@/lib/api/types";

import { AddWidgetMenu } from "./AddWidgetMenu";

/** Reading-log page header — title + the range selector reused
 *  from `/settings/activity`, plus customization actions (Add
 *  widget + Reset to defaults).
 *
 *  The per-event-kind filter chips that originally lived here were
 *  removed: the chrono_feed widget now owns kind selection in its
 *  Configure dialog, which is the surface that actually consumes
 *  the filter. Other widgets either hard-code their kinds
 *  (SeriesFinishes, RecentBookmarks) or ignore the page-level
 *  setting, so the page chips were duplicating work that nothing
 *  observed. */
export function LogHeader({
  range,
  onRangeChange,
  widgets,
}: {
  range: ReadingStatsRange;
  onRangeChange: (next: ReadingStatsRange) => void;
  /** Current widget list — drives the Add-widget menu's filtering
   *  so each kind appears once (except `note`, which is multi-
   *  instance-permitted). */
  widgets: LogWidgetView[];
}) {
  const reset = useResetLogWidgets();
  const [resetOpen, setResetOpen] = React.useState(false);

  return (
    <>
      <PageHeader
        title="Reading log"
        description="Every issue read, series finished, session, and bookmark — in order, with everything Folio knows about each one."
        actions={
          <>
            <ActivityRangeSelector value={range} onChange={onRangeChange} />
            <AddWidgetMenu current={widgets} />
            {/* Data liberation (3.3): download the full reading history as
                CSV. A plain same-origin `<a download>` carries the session
                cookie, so no client fetch/auth plumbing is needed. */}
            <Button
              variant="ghost"
              size="sm"
              asChild
              title="Download your full reading log as a CSV"
            >
              <a href="/api/me/reading-log/export" download>
                <Download aria-hidden="true" className="mr-1 h-3.5 w-3.5" />
                Export CSV
              </a>
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setResetOpen(true)}
              title="Reset to default layout"
            >
              <RotateCcw aria-hidden="true" className="mr-1 h-3.5 w-3.5" />
              Reset
            </Button>
          </>
        }
      />
      <AlertDialog open={resetOpen} onOpenChange={setResetOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Reset to default layout?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes every widget on your reading log and re-adds the built-in
              four: activity feed, at-a-glance, heatmap, and top creators. Your
              reading history isn&rsquo;t touched.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => reset.mutate()}>
              Reset
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
