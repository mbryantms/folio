import { Skeleton } from "@/components/ui/skeleton";

/**
 * `(admin)` group fallback. Renders inside `AdminShell`. Admin pages
 * share the `PageHeader` + (tabs or table) shape, so mirror that: a
 * header with an action slot, a tab bar, and a table silhouette — far
 * closer than a cover grid (admin surfaces like Users are tables).
 */
export default function AdminLoading() {
  return (
    <div aria-hidden>
      {/* PageHeader */}
      <div className="border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-8 w-52" />
          <Skeleton className="h-4 w-72" />
        </div>
        <Skeleton className="h-9 w-32 rounded-md" />
      </div>

      {/* Tab bar */}
      <div className="mb-4 flex flex-wrap gap-2">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton key={i} className="h-8 w-28 rounded-md" />
        ))}
      </div>

      {/* Table */}
      <div className="border-border overflow-hidden rounded-lg border">
        <div className="bg-muted/40 h-10 w-full" />
        <div className="divide-border divide-y">
          {Array.from({ length: 8 }, (_, i) => (
            <div key={i} className="flex items-center gap-4 px-4 py-3">
              <Skeleton className="h-4 w-1/4" />
              <Skeleton className="h-4 w-1/3" />
              <Skeleton className="ml-auto h-4 w-16" />
            </div>
          ))}
        </div>
      </div>
      <span className="sr-only">Loading…</span>
    </div>
  );
}
