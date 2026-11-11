import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { PageRailsSkeleton } from "@/components/skeletons/LoadingShapes";

/** `/pages/<slug>` fallback, shaped like `PageRails`: the page heading
 *  (title + description) and its View options / `⋯` row, then rails of
 *  series cards with the "View all" tail. Without it the route inherited
 *  the `(library)` cover grid. */
export default function CustomPageLoading() {
  return (
    <>
      <LoadingWatchdog />
      <PageRailsSkeleton rails={3} card="series" label="Loading page…" />
    </>
  );
}
