"use client";

import * as React from "react";
import { BookOpen, RotateCcw } from "lucide-react";

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
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
    <header className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex items-center gap-2.5">
          <BookOpen className="text-muted-foreground h-5 w-5" />
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">
              Reading log
            </h1>
            <p className="text-muted-foreground text-sm">
              Every issue read, series finished, session, and bookmark — in
              order, with everything Folio knows about each one.
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ActivityRangeSelector value={range} onChange={onRangeChange} />
          <AddWidgetMenu current={widgets} />
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setResetOpen(true)}
            title="Reset to default layout"
          >
            <RotateCcw aria-hidden="true" className="mr-1 h-3.5 w-3.5" />
            Reset
          </Button>
        </div>
      </div>
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
    </header>
  );
}
