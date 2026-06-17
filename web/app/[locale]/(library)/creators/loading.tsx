import { CreatorGridSkeleton } from "@/components/library/CreatorsIndex";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Creators fallback. The real `CreatorsIndex` is a PageHeader + a search
 * box + an A–Z jump rail + a grid of compact name/role cards — nothing
 * like the inherited `(library)` cover grid. Mirror that exact chrome so
 * navigating here doesn't flash a cover grid, and reuse the same
 * `CreatorGridSkeleton` the component shows once it mounts (so the
 * route-fallback → client-skeleton handoff is seamless).
 */
export default function CreatorsLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* PageHeader */}
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-40" />
          <Skeleton className="h-4 w-72" />
        </div>
      </div>

      {/* Search box */}
      <Skeleton className="h-10 w-full rounded-md" />

      {/* A–Z jump rail */}
      <div className="flex flex-wrap gap-1">
        {Array.from({ length: 14 }, (_, i) => (
          <Skeleton key={i} className="h-7 w-7 rounded-md" />
        ))}
      </div>

      <CreatorGridSkeleton />
      <span className="sr-only">Loading creators…</span>
    </div>
  );
}
