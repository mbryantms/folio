import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { PublisherDetailSkeleton } from "@/components/skeletons/LoadingShapes";

/** `/publishers/<slug>` fallback, shaped like `EntityDetail` for a
 *  publisher: header ("N series") straight over the series cover grid (no
 *  profile card, no tabs). Without it the page inherited the index
 *  skeleton or the `(library)` cover grid. */
export default function PublisherDetailLoading() {
  return (
    <>
      <LoadingWatchdog />
      <PublisherDetailSkeleton label="Loading publisher…" />
    </>
  );
}
