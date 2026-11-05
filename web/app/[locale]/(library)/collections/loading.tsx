import { CollectionsGridSkeleton } from "@/components/collections/CollectionsIndex";
import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import {
  LoadingRegion,
  PageHeaderSkeleton,
  TextLineSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Collections fallback, shaped like `CollectionsIndex`: the PageHeader,
 * then `GroupedViewsBoard`'s toolbar ("Hand-picked series and issues."
 * on the left; search · sort · New group · New collection on the right)
 * over the fixed-width card grid, reusing `CollectionsGridSkeleton`.
 */
export default function CollectionsLoading() {
  return (
    <>
      <LoadingWatchdog />
      <LoadingRegion label="Loading collections…" className="space-y-6">
        <PageHeaderSkeleton
          titleClassName="w-40"
          descriptionClassName="w-[36rem]"
        />
        <div className="space-y-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <TextLineSkeleton className="w-48" />
            <div className="flex flex-wrap items-center gap-2 max-sm:w-full">
              <Skeleton className="h-9 w-56 rounded-md max-sm:w-full max-sm:basis-full" />
              <Skeleton className="h-9 w-[11.5rem] rounded-md" />
              <Skeleton className="h-9 w-28 rounded-md" />
              <Skeleton className="h-9 w-36 rounded-md" />
            </div>
          </div>
          <CollectionsGridSkeleton />
        </div>
      </LoadingRegion>
    </>
  );
}
