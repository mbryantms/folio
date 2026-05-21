import { ActivityReportPage } from "@/components/log/ActivityReportPage";

/** `/log/activity` — the full activity log, reached by clicking the
 *  Activity widget's title on `/log`. Same reverse-chronological
 *  feed but presented as a card-grid report (larger covers, full
 *  credits) instead of the compact widget rows. Spec at
 *  ~/.claude/plans/reading-log.md. */
export default function ActivityReportRoute() {
  return <ActivityReportPage />;
}
