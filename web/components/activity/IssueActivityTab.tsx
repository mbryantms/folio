"use client";

import { ActivityTimeline } from "@/components/activity/ActivityTimeline";
import { IssueActivityStrip } from "@/components/activity/IssueActivityStrip";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";

/**
 * Issue-scoped Activity tab. Mirrors the layout of `<SeriesActivityTab>`:
 * the headline stat cards sit at the top, followed by the per-read
 * timeline. The strip and timeline share the issue scope, so any read
 * the user has just completed shows up in both the moment they return.
 */
export function IssueActivityTab({
  issueId,
  pageCount,
}: {
  issueId: string;
  /** From the issue detail row; lets the avg-per-page card caption the
   *  issue's total length. */
  pageCount: number | null;
}) {
  const stats = useReadingStats({ type: "issue", id: issueId }, "all");

  if (stats.isLoading) return <Skeleton className="h-48 w-full" />;
  if (!stats.data) {
    return (
      <p className="text-destructive text-sm">Failed to load issue stats.</p>
    );
  }

  return (
    <div className="space-y-6">
      <IssueActivityStrip stats={stats.data} pageCount={pageCount} />

      <section>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Per-read history
        </h3>
        <ActivityTimeline
          scope={{ type: "issue", id: issueId }}
          emptyHint="No sessions recorded for this issue yet."
        />
      </section>
    </div>
  );
}
