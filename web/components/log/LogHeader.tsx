"use client";

import * as React from "react";
import {
  BookOpen,
  Check,
  ListChecks,
  MessageSquare,
  RotateCcw,
  Timer,
} from "lucide-react";

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
import { cn } from "@/lib/utils";
import type {
  LogWidgetView,
  ReadingLogEventKind,
  ReadingStatsRange,
} from "@/lib/api/types";

import { AddWidgetMenu } from "./AddWidgetMenu";

const KIND_META: ReadonlyArray<{
  value: ReadingLogEventKind;
  label: string;
  Icon: typeof Check;
}> = [
  { value: "issue_finished", label: "Issues finished", Icon: Check },
  { value: "series_finished", label: "Series finished", Icon: ListChecks },
  { value: "session_completed", label: "Sessions", Icon: Timer },
  { value: "marker_created", label: "Markers", Icon: MessageSquare },
];

/** Reading-log page header — title + count blurb, the global range
 *  selector reused from `/settings/activity`, kind-filter chips,
 *  and the customization actions (Add widget + Reset to defaults).
 *  The page owns the state; the header is purely controlled. */
export function LogHeader({
  range,
  onRangeChange,
  kinds,
  onKindsChange,
  widgets,
}: {
  range: ReadingStatsRange;
  onRangeChange: (next: ReadingStatsRange) => void;
  kinds: ReadingLogEventKind[];
  onKindsChange: (next: ReadingLogEventKind[]) => void;
  /** Current widget list — drives the Add-widget menu's "already
   *  there, hide" filtering. Empty array (e.g. while loading) lets
   *  every kind be addable, which is fine because the mutation
   *  invalidates the list afterward. */
  widgets: LogWidgetView[];
}) {
  const reset = useResetLogWidgets();
  const [resetOpen, setResetOpen] = React.useState(false);
  const toggle = (k: ReadingLogEventKind) => {
    if (kinds.includes(k)) {
      // Leaving the chip set empty would silence the feed; treat the
      // final unclick as a "reset to all kinds" rather than a wipe so
      // the user doesn't end up on an unexpectedly empty page.
      const next = kinds.filter((x) => x !== k);
      onKindsChange(next.length === 0 ? KIND_META.map((m) => m.value) : next);
    } else {
      onKindsChange([...kinds, k]);
    }
  };

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
      <div
        role="group"
        aria-label="Event kind filters"
        className="flex flex-wrap items-center gap-1.5"
      >
        {KIND_META.map(({ value, label, Icon }) => {
          const active = kinds.includes(value);
          return (
            <Button
              key={value}
              variant={active ? "default" : "outline"}
              size="sm"
              onClick={() => toggle(value)}
              aria-pressed={active}
              className={cn(
                "h-7 px-2.5 text-xs",
                !active && "text-muted-foreground",
              )}
            >
              <Icon aria-hidden="true" className="mr-1.5 h-3 w-3" />
              {label}
            </Button>
          );
        })}
      </div>
    </header>
  );
}
