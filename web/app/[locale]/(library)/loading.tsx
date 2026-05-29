import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * `(library)` group fallback. Renders as the child of `MainShell` (the
 * group layout), so it appears inside the real sidebar + topbar — only
 * the page body is skeletonized.
 *
 * Shape mirrors the default landing surface (`/` → `PageRails`): a title
 * + toolbar header, then a vertical stack of horizontal rails. This is a
 * far closer match than the old flat cover grid, which morphed into rails
 * on hydrate. (The `?library=` grid variant of this route reads as a
 * stack of dense rows here, which is acceptable for the brief fallback.)
 */
export default function LibraryLoading() {
  return (
    <div>
      {/* Page heading + toolbar row (mirrors PageRails header). */}
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
        className="flex flex-col"
        style={{ gap: "var(--density-rail-gap)" }}
        aria-hidden
      >
        {Array.from({ length: 3 }, (_, i) => (
          <RailSkeleton key={i} />
        ))}
      </div>
      <span className="sr-only">Loading…</span>
    </div>
  );
}

/** One horizontal rail: a heading line + a row of cover-card skeletons.
 *  `overflow-hidden` clips the row to the viewport like the real rail. */
function RailSkeleton() {
  return (
    <section className="space-y-3">
      <Skeleton className="h-5 w-40" />
      <div className="flex gap-4 overflow-hidden">
        {Array.from({ length: 7 }, (_, i) => (
          <div key={i} className="w-[160px] shrink-0">
            <SeriesCardSkeleton />
          </div>
        ))}
      </div>
    </section>
  );
}
