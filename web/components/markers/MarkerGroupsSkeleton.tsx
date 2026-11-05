import {
  TextLineSkeleton,
  lineWidth,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * `MarkersList` body while it loads: series groups of full-width marker
 * rows. Shared by the `/bookmarks` route fallback and the list's own
 * loading branch so the handoff between them holds still.
 */
export function MarkerGroupsSkeleton({
  groups = [3, 2],
}: {
  /** Rows per group. */
  groups?: number[];
}) {
  return (
    <div className="space-y-8">
      {groups.map((rows, g) => (
        <section key={g} className="space-y-3">
          <TextLineSkeleton
            lineClassName="h-4"
            className={cn("h-2.5", g ? "w-40" : "w-32")}
          />
          {Array.from({ length: rows }, (_, r) => (
            <MarkerRowSkeleton key={r} lines={3 + ((r + g) % 2)} />
          ))}
        </section>
      ))}
    </div>
  );
}

/** `PageNoteRow` / `ClosingNoteRow`: a bordered row with the 56×84 page
 *  thumb, a badge + issue line with the date on the right, then the
 *  snippet's text-xs lines. */
function MarkerRowSkeleton({ lines }: { lines: number }) {
  return (
    <div className="border-border bg-card flex gap-3.5 rounded-lg border p-3">
      <Skeleton className="h-[84px] w-14 shrink-0 rounded" />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex h-7 items-center gap-x-2.5">
          <Skeleton className="h-[18px] w-16 rounded-full" />
          <Skeleton className="h-3 w-40 max-w-[40%]" />
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            <Skeleton className="h-3 w-20" />
            <Skeleton className="h-7 w-7 rounded" />
          </span>
        </div>
        <div>
          {Array.from({ length: lines }, (_, i) => (
            <TextLineSkeleton
              key={i}
              lineClassName="h-4"
              className={cn("h-2.5", i === lines - 1 ? lineWidth(i) : "w-full")}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
