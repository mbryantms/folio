import { CollectionsGridSkeleton } from "@/components/collections/CollectionsIndex";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Collections fallback. `CollectionsIndex` is a responsive **card grid**
 * (PageHeader + "New collection" action, then bordered name/description
 * cards) — so mirror that exact shape via the shared
 * `CollectionsGridSkeleton`, not a list of rows.
 */
export default function CollectionsLoading() {
  return (
    <div className="space-y-6" aria-hidden>
      {/* PageHeader + create action */}
      <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-7 w-44" />
          <Skeleton className="h-4 w-72" />
        </div>
        <Skeleton className="h-9 w-36 rounded-md" />
      </div>

      <CollectionsGridSkeleton />
      <span className="sr-only">Loading collections…</span>
    </div>
  );
}
