import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { LoadingRegion } from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * `/log/year/<year>` fallback, shaped like the full-screen Year in review
 * story (`YearStory`): slide progress bars, the title row and the rounded
 * story stage with its centred skeleton, over the same accent glow.
 * Without it the route inherited the log Overview tiles.
 */
export default function YearInReviewLoading() {
  return (
    <div className="bg-background text-foreground fixed inset-0 z-50 flex flex-col overflow-hidden">
      <LoadingWatchdog />
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 bg-[radial-gradient(60%_50%_at_50%_0%,color-mix(in_oklab,var(--primary)_22%,transparent),transparent),radial-gradient(50%_40%_at_100%_100%,color-mix(in_oklab,var(--primary)_12%,transparent),transparent)]"
      />
      <LoadingRegion
        label="Loading your year in review…"
        rootClassName="relative flex min-h-0 flex-1 flex-col"
        className="relative mx-auto flex min-h-0 w-full max-w-xl flex-1 flex-col px-3 pt-[max(0.75rem,var(--safe-top))] pb-[max(0.75rem,var(--safe-bottom))] sm:px-4 sm:py-6"
      >
        <div className="space-y-2">
          <div className="flex gap-1">
            {Array.from({ length: 7 }, (_, i) => (
              <div key={i} className="flex-1 py-2">
                <div className="bg-foreground/15 h-1 rounded-full" />
              </div>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <Skeleton className="h-4 w-40" />
            <span className="flex-1" />
            <Skeleton className="h-8 w-20 rounded-md" />
            <Skeleton className="size-9 rounded-md" />
          </div>
        </div>
        <div className="bg-card relative mt-2 flex min-h-0 flex-1 flex-col items-center justify-center gap-4 overflow-hidden rounded-3xl border p-8 shadow-[0_30px_80px_-40px_color-mix(in_oklab,var(--primary)_60%,transparent)]">
          <Skeleton className="h-4 w-28" />
          <Skeleton className="h-24 w-56 rounded-2xl" />
          <Skeleton className="h-4 w-40" />
        </div>
      </LoadingRegion>
    </div>
  );
}
