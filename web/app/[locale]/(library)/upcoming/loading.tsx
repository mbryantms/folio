import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import {
  CoverGridSkeleton,
  LoadingRegion,
  PageHeaderSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Upcoming fallback, shaped like `UpcomingView`: the PageHeader, the
 * toolbar row (My series | All releases · filter chips · publisher on the
 * left; Covers | List · View options on the right), the source line, then
 * a week heading over the cover grid. Below `sm` the header and toolbar
 * collapse into the one title + icons bar with the scope toggle under it.
 */
export default function UpcomingLoading() {
  return (
    <>
      <LoadingWatchdog />
      <LoadingRegion label="Loading upcoming releases…" className="space-y-6">
        <PageHeaderSkeleton
          phoneDescription="hidden"
          titleClassName="w-36"
          descriptionClassName="w-96"
          className="max-sm:hidden"
        />

        {/* `StickyToolbarRow` (sm and up). */}
        <div className="-mx-4 flex flex-wrap items-center gap-2 px-4 py-2 max-sm:hidden md:-mx-8 md:px-8">
          <Skeleton className="h-9 w-52 rounded-md" />
          <Skeleton className="h-[22px] w-20 rounded-full" />
          <Skeleton className="h-[22px] w-20 rounded-full" />
          <Skeleton className="h-9 w-48 rounded-md" />
          <div className="ml-auto flex items-center gap-2">
            <Skeleton className="h-9 w-[4.5rem] rounded-md" />
            <Skeleton className="h-9 w-[38px] rounded-md" />
          </div>
        </div>

        {/* Phone bar: title · filters / View options. */}
        <div className="border-border -mx-4 -mt-6 border-b px-4 py-2 sm:hidden">
          <div className="flex items-center gap-2">
            <div className="flex h-7 items-center">
              <Skeleton className="h-5 w-28" />
            </div>
            <div className="ml-auto flex shrink-0 items-center gap-1">
              <Skeleton className="h-9 w-9 rounded-md" />
              <Skeleton className="h-9 w-9 rounded-md" />
            </div>
          </div>
        </div>
        <Skeleton className="h-9 w-full rounded-md sm:hidden" />

        <Skeleton className="h-4 w-64" />

        <div className="space-y-3">
          <Skeleton className="h-7 w-48" />
          <CoverGridSkeleton count={12} label="" />
        </div>
      </LoadingRegion>
    </>
  );
}
