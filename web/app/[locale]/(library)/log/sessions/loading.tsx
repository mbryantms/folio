import { SessionsPageSkeleton } from "@/components/log/LogSkeletons";

/**
 * Sessions tab fallback (without it the route inherited the Overview
 * skeleton): the intro line over the bordered, day-grouped session list.
 */
export default function ReadingSessionsLoading() {
  return <SessionsPageSkeleton />;
}
