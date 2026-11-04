import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import {
  LoadingRegion,
  PageHeaderSkeleton,
  RailTrackSkeleton,
  TextLineSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/** Matches `CARD_WIDTH_PX` in the page's `RoleRail`. */
const CARD_WIDTH_PX = 180;

/**
 * `/creators/<slug>` fallback. The page is a PageHeader ("N series across
 * N roles", role pills on the right) over one "As <role>  N series"
 * section per role, each a horizontal rail of 180px series cards; without
 * this it inherited the creators *index* skeleton (search box + A–Z +
 * name cards).
 */
export default function CreatorDetailLoading() {
  return (
    <>
      <LoadingWatchdog />
      <LoadingRegion label="Loading creator…" className="space-y-8">
        <PageHeaderSkeleton
          titleClassName="w-48"
          descriptionClassName="w-44"
          actions={
            <div className="flex flex-wrap items-center gap-1.5">
              {["w-20", "w-24"].map((w) => (
                <Skeleton key={w} className={`h-[26px] rounded-full ${w}`} />
              ))}
            </div>
          }
        />
        {["w-24", "w-28"].map((w) => (
          <section key={w} className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <TextLineSkeleton lineClassName="h-6" className={`h-4 ${w}`} />
              <Skeleton className="h-3 w-14" />
            </div>
            <RailTrackSkeleton>
              {Array.from({ length: 8 }, (_, i) => (
                <div
                  key={i}
                  className="shrink-0"
                  style={{ width: `${CARD_WIDTH_PX}px` }}
                >
                  <SeriesCardSkeleton />
                </div>
              ))}
            </RailTrackSkeleton>
          </section>
        ))}
      </LoadingRegion>
    </>
  );
}
