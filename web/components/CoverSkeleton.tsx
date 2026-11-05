import { cn } from "@/lib/utils";

/**
 * Loading stand-in for a cover (folio-flair-1.0 "cover-shaped skeletons").
 * A 2:3 tile drawn like a comic cover in silhouette: a masthead band, an
 * issue-number corner and a spine shadow, under the shared shimmer sweep
 * (`.skeleton-shimmer` in globals.css), so a loading grid already reads as
 * a shelf of comics rather than a stack of grey boxes.
 */
export function CoverSkeleton({ className }: { className?: string }) {
  return (
    <div
      aria-hidden
      data-slot="skeleton"
      className={cn(
        "skeleton-shimmer cover-skeleton relative aspect-[2/3] w-full overflow-hidden rounded-md",
        className,
      )}
    >
      <span className="cover-skeleton__masthead" />
      <span className="cover-skeleton__number" />
      <span className="cover-skeleton__spine" />
    </div>
  );
}
