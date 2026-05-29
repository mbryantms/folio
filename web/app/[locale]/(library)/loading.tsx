import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * `(library)` group fallback. Renders as the child of `MainShell` (the
 * group layout), so it appears inside the real sidebar + topbar — only
 * the page body is skeletonized.
 *
 * This route serves both `/` (rails) and `/?library=…` (a cover grid),
 * and `loading.tsx` can't read the query to tell them apart — so the
 * skeleton is a neutral responsive grid of cover cards. Both surfaces are
 * built from cover cards, so it reads as "content loading" for either
 * without committing to the rails shape (which mismatched the grid page).
 */
export default function LibraryLoading() {
  return (
    <div>
      {/* Page heading + toolbar row. */}
      <div
        className="flex flex-wrap items-center justify-between gap-4"
        style={{ marginBottom: "var(--density-page-pad-y)" }}
      >
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-48" />
          <Skeleton className="h-4 w-64" />
        </div>
        <div className="flex items-center gap-2">
          <Skeleton className="h-9 w-28 rounded-md" />
          <Skeleton className="h-9 w-9 rounded-md" />
        </div>
      </div>

      <div
        className="grid gap-4"
        style={{ gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))" }}
        aria-hidden
      >
        {Array.from({ length: 18 }, (_, i) => (
          <SeriesCardSkeleton key={i} />
        ))}
      </div>
      <span className="sr-only">Loading…</span>
    </div>
  );
}
