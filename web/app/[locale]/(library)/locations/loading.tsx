import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexSkeleton } from "@/components/skeletons/EntityIndexSkeleton";

/** `/locations` fallback: the entity index's own chrome, not the inherited
 *  `(library)` cover grid (see `EntityIndexSkeleton`). */
export default function LocationsLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityIndexSkeleton label="Loading locations…" />
    </>
  );
}
