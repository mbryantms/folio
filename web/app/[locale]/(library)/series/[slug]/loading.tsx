import { IssueCardSkeleton } from "@/components/library/IssueCard";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Series-detail fallback. Renders inside `MainShell`, mirroring the real
 * page's structure ([slug]/page.tsx): breadcrumb → hero (cover + title +
 * facts) → 4-up stats → tab bar → issue grid. A flat cover grid (the old
 * inherited fallback) looked nothing like this layered page.
 */
export default function SeriesDetailLoading() {
  return (
    <div className="space-y-10" aria-hidden>
      {/* Breadcrumb */}
      <Skeleton className="h-3 w-40" />

      {/* Hero: cover + buttons | title + facts + chips + summary */}
      <header className="grid grid-cols-1 gap-6 sm:gap-8 lg:grid-cols-[18rem_1fr]">
        <div className="flex flex-col gap-3 sm:gap-4">
          <div className="mx-auto w-4/5 max-w-full sm:w-56 lg:mx-0 lg:w-72">
            <Skeleton className="aspect-[2/3] w-full" />
          </div>
          <div className="mx-auto flex w-full max-w-xs flex-row gap-2 sm:max-w-sm sm:flex-col lg:mx-0 lg:max-w-72">
            <Skeleton className="h-12 flex-1 rounded-md sm:h-10" />
            <Skeleton className="h-12 w-12 rounded-md sm:h-10" />
          </div>
        </div>

        <div className="min-w-0 space-y-5">
          <div className="space-y-3">
            <Skeleton className="h-9 w-3/4" />
            <Skeleton className="h-4 w-1/2" />
            <div className="flex flex-wrap gap-2">
              <Skeleton className="h-6 w-20 rounded-full" />
              <Skeleton className="h-6 w-16 rounded-full" />
              <Skeleton className="h-6 w-24 rounded-full" />
            </div>
          </div>
          <div className="space-y-2">
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-2/3" />
          </div>
          <div className="grid gap-x-6 gap-y-4 sm:grid-cols-2">
            {Array.from({ length: 4 }, (_, i) => (
              <div key={i} className="space-y-2">
                <Skeleton className="h-3 w-24" />
                <Skeleton className="h-4 w-3/4" />
              </div>
            ))}
          </div>
        </div>
      </header>

      {/* Stats */}
      <section className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton key={i} className="h-20 w-full rounded-lg" />
        ))}
      </section>

      {/* Tab bar */}
      <div className="border-border flex flex-wrap gap-2 border-b pb-2">
        {Array.from({ length: 6 }, (_, i) => (
          <Skeleton key={i} className="h-8 w-24 rounded-md" />
        ))}
      </div>

      {/* Issue grid */}
      <ul
        role="list"
        className="grid gap-4"
        style={{
          gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
        }}
      >
        {Array.from({ length: 12 }, (_, i) => (
          <li key={i}>
            <IssueCardSkeleton />
          </li>
        ))}
      </ul>
      <span className="sr-only">Loading series…</span>
    </div>
  );
}
