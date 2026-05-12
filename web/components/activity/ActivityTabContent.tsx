"use client";

import { ActivityStats } from "./ActivityStats";
import { ActivityTimeline } from "./ActivityTimeline";
import { TopRankings } from "./TopRankings";
import type { ReadingStatsScope } from "@/lib/api/queries";

/**
 * Activity tab payload for the series and issue pages. Always renders
 * stats + timeline; series-scope additionally renders top genres / tags
 * (top_series + top_publishers are empty in this scope but the rankings
 * component handles that). Issue-scope skips top rankings entirely since
 * a single issue's metadata is on the Details tab.
 */
export function ActivityTabContent({ scope }: { scope: ReadingStatsScope }) {
  const range = "90d" as const;
  return (
    <div className="space-y-6">
      <ActivityStats scope={scope} range={range} showRangeSelector={false} />
      {scope.type === "series" ? (
        <div>
          <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
            Top genres &amp; tags in this series
          </h3>
          <TopRankings scope={scope} range={range} />
        </div>
      ) : null}
      <div>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Recent sessions
        </h3>
        <ActivityTimeline
          scope={scope}
          emptyHint={
            scope.type === "issue"
              ? "No reading sessions for this issue yet."
              : "No reading sessions for this series yet — start an issue to record your first."
          }
        />
      </div>
    </div>
  );
}
