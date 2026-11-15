import { UpcomingView } from "@/components/upcoming/UpcomingView";

/** Upcoming releases, week by week (upcoming-releases 1.0). View only.
 *
 *  Not prefetched on the server: "this week" depends on the viewer's
 *  local date, which only the browser knows, so the queries start after
 *  hydration (`UpcomingView`). */
export default function UpcomingPage() {
  return <UpcomingView />;
}
