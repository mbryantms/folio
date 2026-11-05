import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { CustomPageRailsSkeleton } from "@/components/skeletons/RouteCardGrid";

/** `/pages/<slug>` fallback, shaped like `PageRails`: the page heading
 *  (title + description) and its View options / `⋯` row, then rails of
 *  series cards with the "View all" tail. Without it the route inherited
 *  the `(library)` cover grid. */
export default function CustomPageLoading() {
  return (
    <>
      <LoadingWatchdog />
      <CustomPageRailsSkeleton />
    </>
  );
}
