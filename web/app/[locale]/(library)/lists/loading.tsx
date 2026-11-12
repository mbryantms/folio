import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import {
  LoadingRegion,
  PageHeaderSkeleton,
  TextLineSkeleton,
} from "@/components/skeletons/LoadingShapes";
import {
  BoardToolbarControlsSkeleton,
  ViewRowsSkeleton,
} from "@/components/skeletons/SettingsShapes";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Reading-lists fallback, shaped like `ReadingListsIndex`: the PageHeader,
 * then `GroupedViewsBoard`'s toolbar (blurb on the left; search · sort ·
 * density · New group · Updates · Import CBL on the right) over the rows
 * table (`ViewRowsSkeleton`, the same shape the board shows while its
 * queries load).
 */
export default function ReadingListsLoading() {
  return (
    <>
      <LoadingWatchdog />
      <LoadingRegion label="Loading reading lists…" className="space-y-6">
        <PageHeaderSkeleton
          titleClassName="w-40"
          descriptionClassName="w-[36rem]"
        />
        <div className="space-y-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <TextLineSkeleton className="w-56" />
            <div className="flex flex-wrap items-center gap-2 max-sm:w-full">
              <BoardToolbarControlsSkeleton density />
              <Skeleton className="h-9 w-9 rounded-md sm:w-24" />
              <Skeleton className="h-9 w-28 rounded-md" />
            </div>
          </div>
          <ViewRowsSkeleton />
        </div>
      </LoadingRegion>
    </>
  );
}
