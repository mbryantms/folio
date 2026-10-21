import { LoadingWatchdog } from "@/components/LoadingWatchdog";
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
    <div className="flex flex-col gap-6 sm:gap-10" aria-hidden>
      <LoadingWatchdog />
      {/* Breadcrumb */}
      <Skeleton className="h-3 w-40" />

      {/* Hero: mirrors page.tsx — on phones a 2fr/3fr grid with the
          cover beside the title block and the Read row spanning both;
          sm+ the stacked cover sidebar, lg+ side by side. */}
      <header className="grid grid-cols-[2fr_3fr] gap-4 sm:grid-cols-1 sm:gap-8 lg:grid-cols-[18rem_1fr]">
        <div className="flex flex-col gap-3 max-sm:contents sm:gap-4">
          <div className="mx-auto w-full max-w-full max-sm:order-1 sm:w-56 lg:mx-0 lg:w-72">
            <Skeleton className="aspect-[2/3] w-full" />
          </div>
          <div className="mx-auto flex w-full max-w-xs flex-row gap-2 max-sm:order-3 max-sm:col-span-full max-sm:max-w-none sm:max-w-sm sm:flex-col lg:mx-0 lg:max-w-72">
            <Skeleton className="h-12 flex-1 rounded-md sm:h-10" />
            <Skeleton className="h-12 w-12 rounded-md sm:h-10" />
          </div>
        </div>

        <div className="min-w-0 max-sm:contents sm:space-y-5">
          <div className="space-y-3 max-sm:order-2">
            <Skeleton className="h-9 w-3/4" />
            <Skeleton className="h-4 w-1/2" />
            <div className="flex flex-wrap gap-2">
              <Skeleton className="h-6 w-20 rounded-full" />
              <Skeleton className="h-6 w-16 rounded-full" />
              <Skeleton className="h-6 w-24 rounded-full" />
            </div>
          </div>
          <div className="space-y-2 max-sm:order-3 max-sm:col-span-full">
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-2/3" />
          </div>
          <div className="grid gap-x-6 gap-y-4 max-sm:order-3 max-sm:col-span-full sm:grid-cols-2">
            {Array.from({ length: 4 }, (_, i) => (
              <div
                key={i}
                className={i < 2 ? "space-y-2 max-sm:hidden" : "space-y-2"}
              >
                <Skeleton className="h-3 w-24" />
                <Skeleton className="h-4 w-3/4" />
              </div>
            ))}
          </div>
        </div>
      </header>

      {/* Stats — a horizontal strip on phones, 4-up from sm. */}
      <section className="grid grid-cols-2 gap-3 max-sm:-mx-4 max-sm:flex max-sm:[scrollbar-width:none] max-sm:overflow-x-auto max-sm:px-4 sm:grid-cols-4">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton
            key={i}
            className="h-20 w-full rounded-lg max-sm:min-w-[9rem] max-sm:shrink-0"
          />
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
