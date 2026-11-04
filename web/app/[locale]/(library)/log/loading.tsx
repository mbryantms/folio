import { Skeleton } from "@/components/ui/skeleton";

/**
 * Reading-log fallback, shaped like the Overview tab (the log layout's
 * header and tabs render around it): four stat tiles, the year banner,
 * the heatmap card and the milestone grid.
 */
export default function ReadingLogLoading() {
  return (
    <div className="space-y-8" aria-hidden>
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-28 rounded-xl" />
        ))}
      </div>
      <Skeleton className="h-32 rounded-xl" />
      <Skeleton className="h-56 rounded-xl" />
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-20 rounded-xl" />
        ))}
      </div>
      <span className="sr-only">Loading reading log…</span>
    </div>
  );
}
