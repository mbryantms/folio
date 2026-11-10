import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import {
  LoadingRegion,
  PageHeaderSkeleton,
  TextLineSkeleton,
} from "@/components/skeletons/LoadingShapes";
import {
  BoardToolbarControlsSkeleton,
  ViewCardsSkeleton,
} from "@/components/skeletons/SettingsShapes";

/**
 * Reading-lists fallback, shaped like `ReadingListsIndex`: the PageHeader,
 * then `GroupedViewsBoard`'s toolbar (blurb on the left; search · sort ·
 * New group on the right) over the compact card grid.
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
              <BoardToolbarControlsSkeleton />
            </div>
          </div>
          <ViewCardsSkeleton />
        </div>
      </LoadingRegion>
    </>
  );
}
