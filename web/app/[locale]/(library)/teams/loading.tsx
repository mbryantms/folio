import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexSkeleton } from "@/components/skeletons/EntityIndexSkeleton";

/** `/teams` fallback: the entity index's own chrome, not the inherited
 *  `(library)` cover grid (see `EntityIndexSkeleton`). */
export default function TeamsLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityIndexSkeleton label="Loading teams…" />
    </>
  );
}
