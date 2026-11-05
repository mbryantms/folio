import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { MarkerGroupsSkeleton } from "@/components/markers/MarkerGroupsSkeleton";
import {
  LoadingRegion,
  PageHeaderSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/** Kind filter chips, sized to their labels: All · Bookmarks · Notes ·
 *  Closing notes · Favorites · Highlights (each with a count). */
const CHIP_WIDTHS = ["w-10", "w-24", "w-16", "w-28", "w-20", "w-24"];

/**
 * Bookmarks fallback, shaped like `MarkersList`: the PageHeader, the
 * toolbar row (filter input · sort · series on the left; Select · View
 * options · `⋯` on the right), the kind chip row with the By series |
 * Newest toggle, then series groups of full-width marker rows (thumb,
 * kind badge + issue line + date, a few snippet lines). Below `sm` the
 * header and toolbar collapse into the one sticky title + icons bar.
 */
export default function BookmarksLoading() {
  return (
    <>
      <LoadingWatchdog />
      <LoadingRegion label="Loading bookmarks…" className="space-y-6">
        <PageHeaderSkeleton
          phoneDescription="hidden"
          titleClassName="w-36"
          descriptionClassName="w-56"
          className="max-sm:hidden"
        />

        {/* `StickyToolbarRow` (sm and up). */}
        <div className="-mx-4 flex flex-wrap items-center gap-2 px-4 py-2 max-sm:hidden md:-mx-8 md:px-8">
          <Skeleton className="h-9 max-w-64 min-w-40 flex-1 basis-40 rounded-md" />
          <Skeleton className="h-9 w-24 rounded-md" />
          <Skeleton className="h-9 w-28 rounded-md" />
          <div className="ml-auto flex items-center gap-2">
            <Skeleton className="h-9 w-[5.5rem] rounded-md" />
            <Skeleton className="h-9 w-[38px] rounded-md" />
            <Skeleton className="h-9 w-9 rounded-md" />
          </div>
        </div>

        {/* Phone bar: title · search / filters / View options / ⋯. */}
        <div className="border-border -mx-4 -mt-6 border-b px-4 py-2 sm:hidden">
          <div className="flex items-center gap-2">
            <div className="flex h-7 items-center">
              <Skeleton className="h-5 w-28" />
            </div>
            <div className="ml-auto flex shrink-0 items-center gap-1">
              {Array.from({ length: 4 }, (_, i) => (
                <Skeleton key={i} className="h-9 w-9 rounded-md" />
              ))}
            </div>
          </div>
        </div>

        {/* Kind chips + By series | Newest toggle. */}
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex flex-wrap gap-1 max-sm:-mx-4 max-sm:min-w-0 max-sm:flex-1 max-sm:flex-nowrap max-sm:overflow-hidden max-sm:px-4 max-sm:[&>*]:shrink-0">
            {CHIP_WIDTHS.map((w, i) => (
              <Skeleton key={i} className={cn("h-[22px] rounded-full", w)} />
            ))}
          </div>
          <Skeleton className="ml-auto h-6 w-[7.5rem] rounded-md max-sm:hidden" />
        </div>

        <MarkerGroupsSkeleton />
      </LoadingRegion>
    </>
  );
}
