"use client";

import * as React from "react";

import { IssueCardSkeleton } from "@/components/library/IssueCard";
import { RailError } from "@/components/saved-views/RailError";
import { UpcomingCard } from "@/components/upcoming/UpcomingCard";
import { UpcomingEntryDialog } from "@/components/upcoming/UpcomingEntryDialog";
import { useUpcomingThisWeek } from "@/lib/api/queries";
import type { UpcomingEntryView } from "@/lib/api/types";
import { RAIL_PREVIEW_LIMIT } from "@/lib/saved-views/rail-queries";
import { localToday } from "@/lib/upcoming/stage";

/** The query behind the "Out this week" rail, shared by the body and by
 *  `useSystemRailIsEmpty` (one cache entry). Keyed by the viewer's local
 *  date, read at render like `useOnThisDay`: rails fetch in the browser
 *  only, so the server's idea of the date never reaches the request. */
export function useUpcomingRail(opts?: { enabled?: boolean }) {
  return useUpcomingThisWeek(localToday(), {
    enabled: opts?.enabled ?? true,
  });
}

/**
 * "Out this week" home rail body: this week's releases in the reader's
 * series, as the same cards the Upcoming page draws. A card opens the
 * read-only release dialog; "View all" leads to `/upcoming`, where the
 * week is listed whole.
 *
 * Hidden when empty (`useSystemRailIsEmpty`), which is also how it
 * disappears while the feature is off. Cards cannot be dismissed one by
 * one: rail dismissals target issues, series and lists in the library,
 * and a release is none of those. The rail is unpinned like any other.
 */
export function UpcomingThisWeekRailBody({
  itemStyle,
}: {
  itemStyle: React.CSSProperties;
}) {
  const q = useUpcomingRail();
  const [selected, setSelected] = React.useState<UpcomingEntryView | null>(
    null,
  );
  const [open, setOpen] = React.useState(false);
  const show = React.useCallback((entry: UpcomingEntryView) => {
    setSelected(entry);
    setOpen(true);
  }, []);

  if (q.isLoading || q.isPending) {
    return (
      <>
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} style={itemStyle} className="shrink-0">
            <IssueCardSkeleton />
          </div>
        ))}
      </>
    );
  }
  if (q.isError && !q.data) {
    return <RailError onRetry={() => void q.refetch()} />;
  }
  const items = q.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <>
      {items.slice(0, RAIL_PREVIEW_LIMIT).map((entry) => (
        <div key={entry.key} style={itemStyle} className="shrink-0">
          <UpcomingCard entry={entry} onOpen={show} />
        </div>
      ))}
      <UpcomingEntryDialog
        entry={selected}
        open={open}
        onOpenChange={setOpen}
      />
    </>
  );
}
