import { StatsTab } from "@/components/log/StatsTab";

/** `/log/stats` — the reading log's Stats tab: range-scoped headline
 *  numbers, weekly rhythm, time of day, activity, rankings, pace and
 *  rereads. Moved here from Settings → Reading stats (folio-flair
 *  phase 4, ~/.claude/plans/folio-flair-phase4-log.md). The log layout
 *  draws the header and tab bar. */
export default function ReadingLogStatsRoute() {
  return <StatsTab />;
}
