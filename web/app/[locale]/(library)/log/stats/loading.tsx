import { StatsPageSkeleton } from "@/components/log/LogSkeletons";

/**
 * Stats tab fallback, under the log layout's header + tabs: the range
 * row, then the same body `StatsTab` shows while its query loads.
 */
export default function ReadingLogStatsLoading() {
  return <StatsPageSkeleton />;
}
