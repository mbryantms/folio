import { OverviewPage } from "@/components/log/overview/OverviewPage";

/** `/log` — the reading log's Overview tab (folio-flair phase 4): this
 *  year's numbers, the year in review, a year of activity, milestones and
 *  recent reading. Header + tabs come from the log layout. Plan:
 *  ~/.claude/plans/folio-flair-phase4-log.md */
export default function ReadingLogRoute() {
  return <OverviewPage />;
}
