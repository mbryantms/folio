import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexSkeleton } from "@/components/skeletons/EntityIndexSkeleton";

/** `/arcs` fallback: the entity index's own chrome, not the inherited
 *  `(library)` cover grid (see `EntityIndexSkeleton`). */
export default function ArcsLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityIndexSkeleton label="Loading story arcs…" />
    </>
  );
}
