import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import {
  LoadingRegion,
  RailsSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * `/creators/<slug>` fallback. The page is a PageHeader with role chips
 * over one horizontal series rail per role; without this it inherited the
 * creators *index* skeleton (search box + A–Z + name cards).
 */
export default function CreatorDetailLoading() {
  return (
    <LoadingRegion label="Loading creator…" className="space-y-8">
      <LoadingWatchdog />
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-48" />
          <Skeleton className="h-4 w-56 max-w-full" />
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {["w-20", "w-24", "w-16"].map((w) => (
            <Skeleton key={w} className={`h-7 rounded-full ${w}`} />
          ))}
        </div>
      </div>
      <RailsSkeleton rails={2} label="" />
    </LoadingRegion>
  );
}
