import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexSkeleton } from "@/components/skeletons/EntityIndexSkeleton";

/**
 * Creators fallback. The real `CreatorsIndex` is the entity-index chrome
 * (PageHeader + search box + A–Z jump rail) over a grid of name/role
 * cards, so it shares `EntityIndexSkeleton`, ending on the same
 * `CreatorGridSkeleton` the component shows once it mounts (so the
 * route-fallback → client-skeleton handoff is seamless).
 */
export default function CreatorsLoading() {
  return (
    <>
      <LoadingWatchdog />
      <EntityIndexSkeleton label="Loading creators…" creators />
    </>
  );
}
