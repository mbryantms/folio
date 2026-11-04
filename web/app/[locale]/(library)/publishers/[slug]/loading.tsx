import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityDetailSkeleton } from "@/components/skeletons/LoadingShapes";

/** `/publishers/<slug>` fallback, shaped like `EntityDetail`: header, blurb and series grid (publishers have no tabs). Without it
 *  the page inherited the index skeleton or the `(library)` cover grid. */
export default function PublisherDetailLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityDetailSkeleton tabs={false} label="Loading publisher…" />
    </>
  );
}
