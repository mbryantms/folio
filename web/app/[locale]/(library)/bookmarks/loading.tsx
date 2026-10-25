import { IssueCardSkeleton } from "@/components/library/IssueCard";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Bookmarks fallback. Mirrors `MarkersList`: a page header, a toolbar
 * (search + kind filter pills + tag pills + density toggle), then the
 * marker card grid. The toolbar silhouette signals the filter
 * affordances before real content arrives. Below `sm` the header and
 * toolbar collapse into the one title + icons row the phone bar draws.
 */
export default function BookmarksLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* PageHeader (sm and up) */}
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4 max-sm:hidden">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-40" />
          <Skeleton className="h-4 w-56" />
        </div>
        <Skeleton className="h-9 w-48 max-w-xs rounded-md" />
      </div>

      {/* Phone bar: title + three icon buttons on one row */}
      <div className="border-border flex items-center justify-between gap-2 border-b py-2 sm:hidden">
        <Skeleton className="h-6 w-32" />
        <div className="flex items-center gap-1">
          {Array.from({ length: 3 }, (_, i) => (
            <Skeleton key={i} className="h-9 w-9 rounded-md" />
          ))}
        </div>
      </div>

      {/* Filter pills + density toggle */}
      <div className="flex flex-wrap items-center gap-2">
        {Array.from({ length: 5 }, (_, i) => (
          <Skeleton key={i} className="h-7 w-20 rounded-full" />
        ))}
        <Skeleton className="ml-auto h-9 w-28 rounded-md" />
      </div>

      {/* Marker grid */}
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
      <span className="sr-only">Loading bookmarks…</span>
    </div>
  );
}
