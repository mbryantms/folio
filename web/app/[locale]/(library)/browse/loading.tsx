import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { EntityIndexBodySkeleton } from "@/components/skeletons/EntityIndexSkeleton";
import {
  LineTabStripSkeleton,
  LoadingRegion,
  PageHeaderSkeleton,
  TextLineSkeleton,
} from "@/components/skeletons/LoadingShapes";

/**
 * `/browse` fallback, shaped like `BrowseTabs`: the "Browse" PageHeader,
 * the underline tab strip (Characters · Teams · Story arcs · Publishers ·
 * Creators), then the embedded index — its one-line count, the search
 * box, the A–Z rail and the name-card grid. Without it the route
 * inherited the `(library)` cover grid.
 */
export default function BrowseLoading() {
  return (
    <>
      <LoadingWatchdog />
      <LoadingRegion label="Loading browse…" className="space-y-6">
        <PageHeaderSkeleton
          phoneDescription="hidden"
          titleClassName="w-28"
          descriptionClassName="w-[28rem]"
        />
        {/* `Tabs` root: flex-col gap-2, content pt-4. */}
        <div className="flex flex-col gap-2">
          <LineTabStripSkeleton
            widths={["w-20", "w-12", "w-20", "w-20", "w-16"]}
          />
          <div className="space-y-6 pt-4">
            <TextLineSkeleton
              lineClassName="h-5 max-sm:hidden"
              className="w-60"
            />
            <EntityIndexBodySkeleton />
          </div>
        </div>
      </LoadingRegion>
    </>
  );
}
