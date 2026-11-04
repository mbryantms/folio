import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexSkeleton } from "@/components/skeletons/EntityIndexSkeleton";

/** `/characters` fallback: the entity index's own chrome, not the inherited
 *  `(library)` cover grid (see `EntityIndexSkeleton`). */
export default function CharactersLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityIndexSkeleton label="Loading characters…" />
    </>
  );
}
