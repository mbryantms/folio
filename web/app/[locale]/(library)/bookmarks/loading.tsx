import { IssueCardSkeleton } from "@/components/library/IssueCard";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Bookmarks fallback. Mirrors `MarkersList`: a page header, a toolbar
 * (search + kind filter pills + tag pills + density toggle), then the
 * marker card grid. The toolbar silhouette signals the filter
 * affordances before real content arrives.
 */
export default function BookmarksLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* PageHeader */}
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-40" />
          <Skeleton className="h-4 w-56" />
        </div>
        <Skeleton className="h-9 w-48 max-w-xs rounded-md" />
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
