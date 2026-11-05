import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityDetailSkeleton } from "@/components/skeletons/LoadingShapes";

/** `/teams/<slug>` fallback, shaped like `EntityDetail`: header, profile card and the "In your library" insights. Without it
 *  the page inherited the index skeleton or the `(library)` cover grid. */
export default function TeamDetailLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityDetailSkeleton label="Loading team…" />
    </>
  );
}
