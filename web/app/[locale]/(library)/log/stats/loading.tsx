import { Skeleton } from "@/components/ui/skeleton";

/**
 * Stats tab fallback, under the log layout's header + tabs: the range row,
 * the headline band and the first two rows of cards, in the same grid
 * `StatsTab` lays out.
 */
export default function ReadingLogStatsLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      <div className="flex items-center justify-between gap-3">
        <Skeleton className="h-4 w-48 max-sm:hidden" />
        <Skeleton className="h-9 w-full rounded-md sm:w-96" />
      </div>
      <div className="border-border bg-border grid grid-cols-2 gap-px overflow-hidden rounded-xl border sm:grid-cols-3 xl:grid-cols-6">
        {Array.from({ length: 6 }, (_, i) => (
          <div key={i} className="bg-card space-y-3 px-4 py-4 sm:px-5 sm:py-5">
            <Skeleton className="h-3 w-16" />
            <Skeleton className="h-8 w-20" />
            <Skeleton className="h-3 w-24" />
          </div>
        ))}
      </div>
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-5">
        <CardSkeleton className="lg:col-span-3" body="h-48" />
        <CardSkeleton className="lg:col-span-2" body="h-48" />
      </div>
      <CardSkeleton body="h-52" />
      <span className="sr-only">Loading reading stats…</span>
    </div>
  );
}

function CardSkeleton({
  className = "",
  body,
}: {
  className?: string;
  body: string;
}) {
  return (
    <div
      className={`border-border bg-card space-y-4 rounded-xl border p-4 sm:p-5 ${className}`}
    >
      <div className="space-y-2">
        <Skeleton className="h-5 w-36" />
        <Skeleton className="h-4 w-64 max-w-full" />
      </div>
      <Skeleton className={`w-full ${body}`} />
    </div>
  );
}
