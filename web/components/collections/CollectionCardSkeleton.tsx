import { CoverSkeleton } from "@/components/CoverSkeleton";
import { Skeleton } from "@/components/ui/skeleton";
import { STACK_ASPECT } from "@/lib/cover-stack/variants";

/** Single placeholder card — mirrors the large `ViewCard` the board
 *  renders for a collection (bg-card tile, the cover-stack stage bleeding
 *  to the card's top edge, name, a description line, a small pill), so
 *  the loading state has the same footprint as the real grid. Exported so
 *  the route `loading.tsx` reuses the exact shape. */
export function CollectionCardSkeleton() {
  return (
    <div className="border-border/60 bg-card flex h-full flex-col gap-3 rounded-lg border p-4">
      {/* `CoverStackStage` (large): the stack box at the shared aspect,
          with a front-cover silhouette centred in it. */}
      <div className="-mx-4 -mt-4 flex justify-center px-3 pt-7 pb-2">
        <div
          className="flex w-full max-w-[22rem] justify-center"
          style={{ aspectRatio: `1 / ${STACK_ASPECT}` }}
        >
          <CoverSkeleton className="h-full w-auto" />
        </div>
      </div>
      <div className="space-y-0.5">
        <div className="flex h-6 items-center">
          <Skeleton className="h-4 w-2/3" />
        </div>
        <div className="flex h-5 items-center">
          <Skeleton className="h-3 w-3/4" />
        </div>
      </div>
      <div className="mt-auto flex items-center gap-1.5 pt-1">
        <Skeleton className="h-5 w-16" />
      </div>
    </div>
  );
}
