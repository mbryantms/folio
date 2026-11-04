import { FinishedPageSkeleton } from "@/components/log/LogSkeletons";

/**
 * Finished tab fallback (without it the route inherited the Overview
 * skeleton): intro + range control + group select, the four stat tiles
 * and the weekly cover feed.
 */
export default function ActivityReportLoading() {
  return <FinishedPageSkeleton />;
}
