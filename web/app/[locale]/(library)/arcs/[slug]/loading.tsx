import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityDetailSkeleton } from "@/components/skeletons/LoadingShapes";

/** `/arcs/<slug>` fallback, shaped like `EntityDetail`: header, profile blurb, Issues/Series tabs and the issue grid. Without it
 *  the page inherited the index skeleton or the `(library)` cover grid. */
export default function ArcDetailLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityDetailSkeleton label="Loading story arc…" />
    </>
  );
}
