"use client";

import * as React from "react";
import {
  BookOpen,
  Check,
  ListChecks,
  MessageSquare,
  Timer,
} from "lucide-react";

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { ReadingLogEventKind, ReadingStatsRange } from "@/lib/api/types";

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
 *  selector reused from `/settings/activity`, and a row of kind-filter
 *  chips. The page owns the state; the header is purely controlled. */
export function LogHeader({
  range,
  onRangeChange,
  kinds,
  onKindsChange,
}: {
  range: ReadingStatsRange;
  onRangeChange: (next: ReadingStatsRange) => void;
  kinds: ReadingLogEventKind[];
  onKindsChange: (next: ReadingLogEventKind[]) => void;
}) {
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
        <ActivityRangeSelector value={range} onChange={onRangeChange} />
      </div>
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
