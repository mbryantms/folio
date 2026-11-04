import { EntityGridSkeleton } from "@/components/library/EntityIndex";
import {
  LoadingRegion,
  PageHeaderSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Route fallback for the entity browse indexes (`/arcs`, `/characters`,
 * `/teams`, `/publishers`, `/locations`). `EntityIndex` is a PageHeader, a
 * search box, an A–Z jump rail and a grid of compact name cards, not the
 * inherited `(library)` cover grid; it ends on the same `EntityGridSkeleton`
 * the component shows while its query loads, so the handoff is seamless.
 */
export function EntityIndexSkeleton({ label }: { label: string }) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <PageHeaderSkeleton />
      <Skeleton className="h-10 w-full rounded-md max-sm:mr-2" />
      <div className="flex flex-wrap gap-1 max-sm:hidden">
        {Array.from({ length: 27 }, (_, i) => (
          <Skeleton key={i} className="h-7 w-7 rounded-md" />
        ))}
      </div>
      <EntityGridSkeleton />
    </LoadingRegion>
  );
}
