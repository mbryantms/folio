import { Skeleton } from "@/components/ui/skeleton";

/**
 * Reading-log fallback. `ReadingLogPage` is a `PageHeader` (range selector
 * + Export CSV + reset) over a masonry-style widget grid — not the
 * inherited `(library)` cover grid. Mirror the header + the same
 * `columns-1 md:columns-2` widget flow `ReadingLogPage` shows while its
 * widgets load, so navigating here doesn't flash a cover grid.
 */
export default function ReadingLogLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* PageHeader: title + range / export / reset actions */}
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-44" />
          <Skeleton className="h-4 w-64" />
        </div>
        <div className="flex items-center gap-2">
          <Skeleton className="h-9 w-32 rounded-md" />
          <Skeleton className="h-9 w-28 rounded-md" />
          <Skeleton className="h-9 w-9 rounded-md" />
        </div>
      </div>

      {/* Widget grid — same multicolumn flow as LogWidgetGrid. */}
      <div className="columns-1 gap-x-6 md:columns-2">
        <Skeleton className="mb-6 inline-block h-64 w-full break-inside-avoid" />
        <Skeleton className="mb-6 inline-block h-48 w-full break-inside-avoid" />
        <Skeleton className="mb-6 inline-block h-56 w-full break-inside-avoid" />
        <Skeleton className="mb-6 inline-block h-32 w-full break-inside-avoid" />
      </div>
      <span className="sr-only">Loading reading log…</span>
    </div>
  );
}
