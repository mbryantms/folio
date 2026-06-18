import { Skeleton } from "@/components/ui/skeleton";

/**
 * Search fallback. `SearchView` is a header (title + card-size / filter
 * controls + the always-focused search input + a result-count summary)
 * over one horizontal-scroll rail per category — NOT the inherited
 * `(library)` cover grid. Mirror that exact chrome (input + a couple of
 * labelled rails) so a deep-link like `/search?q=geiger` doesn't flash a
 * cover grid before the client component hydrates and runs the query.
 */
export default function SearchLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* Header: title + toolbar controls */}
      <div className="space-y-3">
        <div className="flex flex-wrap items-baseline justify-between gap-4">
          <Skeleton className="h-8 w-32" />
          <div className="flex items-center gap-2">
            <Skeleton className="h-9 w-9 rounded-md" />
          </div>
        </div>
        <Skeleton className="h-4 w-80" />
        {/* The search input box */}
        <Skeleton className="h-10 w-full rounded-md" />
        <Skeleton className="h-3 w-44" />
      </div>

      {/* Two category rails — header (label + count) over a row of
          cover-shaped cards. */}
      {[0, 1].map((rail) => (
        <div key={rail} className="space-y-3">
          <div className="flex items-center gap-2">
            <Skeleton className="h-5 w-24" />
            <Skeleton className="h-3 w-12" />
          </div>
          <div className="flex gap-4 overflow-hidden">
            {Array.from({ length: 8 }, (_, i) => (
              <div key={i} className="w-40 shrink-0 space-y-2">
                <Skeleton className="aspect-[2/3] w-full rounded-md" />
                <Skeleton className="h-4 w-3/4" />
                <Skeleton className="h-3 w-1/2" />
              </div>
            ))}
          </div>
        </div>
      ))}
      <span className="sr-only">Loading search…</span>
    </div>
  );
}
