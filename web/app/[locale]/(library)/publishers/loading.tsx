import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexSkeleton } from "@/components/skeletons/EntityIndexSkeleton";

/** `/publishers` fallback: the entity index's own chrome, not the inherited
 *  `(library)` cover grid (see `EntityIndexSkeleton`). */
export default function PublishersLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityIndexSkeleton label="Loading publishers…" />
    </>
  );
}
