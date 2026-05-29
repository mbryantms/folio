import { Skeleton } from "@/components/ui/skeleton";

/**
 * Collections fallback. `CollectionsIndex` is a managed list (header +
 * create action + collection rows), not a browsable cover grid — so this
 * mirrors a header + a stack of list rows rather than a card grid.
 */
export default function CollectionsLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* PageHeader + create action */}
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-44" />
          <Skeleton className="h-4 w-60" />
        </div>
        <Skeleton className="h-9 w-32 rounded-md" />
      </div>

      <div className="space-y-3">
        {Array.from({ length: 6 }, (_, i) => (
          <div
            key={i}
            className="border-border flex items-center gap-4 rounded-lg border p-3"
          >
            <Skeleton className="h-14 w-10 shrink-0 rounded" />
            <div className="min-w-0 flex-1 space-y-2">
              <Skeleton className="h-4 w-1/3" />
              <Skeleton className="h-3 w-1/4" />
            </div>
            <Skeleton className="h-8 w-8 shrink-0 rounded-md" />
          </div>
        ))}
      </div>
      <span className="sr-only">Loading collections…</span>
    </div>
  );
}
