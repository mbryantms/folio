import { CreatorGridSkeleton } from "@/components/library/CreatorsIndex";
import { EntityGridSkeleton } from "@/components/library/EntityIndex";
import {
  LoadingRegion,
  PageHeaderSkeleton,
  TextLineSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * Route fallback for the entity browse indexes (`/arcs`, `/characters`,
 * `/teams`, `/publishers`, `/locations`) and `/creators`. `EntityIndex` /
 * `CreatorsIndex` are a PageHeader, a search box, an A–Z jump rail and a
 * grid of compact name cards, not the inherited `(library)` cover grid;
 * the skeleton ends on the same grid skeleton the component shows while
 * its query loads, so the handoff is seamless.
 */
export function EntityIndexSkeleton({
  label,
  creators = false,
}: {
  label: string;
  /** Creator cards carry a role-chip row (`CreatorGridSkeleton`). */
  creators?: boolean;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <PageHeaderSkeleton
        phoneDescription="hidden"
        titleClassName="w-36"
        descriptionClassName="w-64"
      />
      <EntityIndexBodySkeleton creators={creators} />
    </LoadingRegion>
  );
}

/** Everything under the header: search box, A–Z rail and the card grid.
 *  Shared with `/browse`, whose tabs embed the same index body. Render it
 *  inside a `space-y-6` stack (the real index's container). */
export function EntityIndexBodySkeleton({
  creators = false,
}: {
  creators?: boolean;
}) {
  return (
    <>
      <SearchBoxSkeleton />
      <AtoZRailSkeleton className="max-sm:hidden" />
      <div className="max-sm:pr-2">
        {creators ? <CreatorGridSkeleton /> : <EntityGridSkeleton />}
      </div>
    </>
  );
}

/** The index search box: same border, padding and line box as the real
 *  `<input>` row, with the icon and a placeholder bar inside. */
export function SearchBoxSkeleton({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "border-border bg-card flex items-center gap-2 rounded-md border px-3 py-2 shadow-sm max-sm:mr-2 max-sm:py-1.5 max-sm:shadow-none",
        className,
      )}
    >
      <Skeleton className="size-4 shrink-0 rounded-sm" />
      <TextLineSkeleton className="h-3 w-48 max-w-full" />
    </div>
  );
}

/** `AtoZJumpRail` stand-in: `#` + A–Z as small letter marks in the rail's
 *  own 24px cells, spread edge-to-edge from `md` like the real strip. */
export function AtoZRailSkeleton({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-0.5 md:flex-nowrap md:justify-between",
        className,
      )}
    >
      {Array.from({ length: 27 }, (_, i) => (
        <div key={i} className="grid size-6 place-items-center">
          <Skeleton className="h-3 w-2 rounded-sm" />
        </div>
      ))}
    </div>
  );
}
