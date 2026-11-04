import { OverviewSkeleton } from "@/components/log/LogSkeletons";

/**
 * Reading-log fallback, shaped like the Overview tab (the log layout's
 * header and tabs render around it). The same shape is `OverviewPage`'s
 * own loading state, so the handoff holds still.
 */
export default function ReadingLogLoading() {
  return <OverviewSkeleton />;
}
