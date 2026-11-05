import { CoverSkeleton } from "@/components/CoverSkeleton";
import { Skeleton } from "@/components/ui/skeleton";

/** Category rails while search results load: shared by the route
 *  fallback and `SearchView`'s own "searching" state. */
export function SearchRailsSkeleton() {
  return (
    <div aria-hidden className="space-y-6">
      {/* Two category rails — header (label + count) over a row of
        cover-shaped cards. */}
      {[0, 1].map((rail) => (
        <div key={rail} className="space-y-3">
          <div className="flex items-center gap-2">
            <Skeleton className="h-5 w-24" />
            <Skeleton className="h-3 w-12" />
          </div>
          <div className="flex gap-4 overflow-hidden">
            {Array.from({ length: 8 }, (_, i) => (
              <div key={i} className="w-40 shrink-0 space-y-2">
                <CoverSkeleton />
                <Skeleton className="h-4 w-3/4" />
                <Skeleton className="h-3 w-1/2" />
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
