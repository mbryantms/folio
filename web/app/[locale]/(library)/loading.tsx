import { Suspense } from "react";

import { LoadingWatchdog } from "@/components/LoadingWatchdog";
import { MorphAwareLoading } from "@/components/library/MorphAwareLoading";
import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { LibraryQuerySwitch } from "@/components/skeletons/LibraryQuerySwitch";
import { PageRailsSkeleton } from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

import SeriesDetailLoading from "./series/[slug]/loading";
import { CARD_SIZE_KEYS } from "@/lib/library/card-size";
import { RouteCardGrid } from "@/components/skeletons/RouteCardGrid";

/**
 * `(library)` group fallback. Renders as the child of `MainShell` (the
 * group layout), so it appears inside the real sidebar + topbar — only
 * the page body is skeletonized.
 *
 * This route serves both `/` (Home: a heading over saved-view rails) and
 * `/?library=…` (a cover grid). `loading.tsx` can't read the query, so
 * `LibraryQuerySwitch` (a client component) picks the shape from the URL;
 * it sits under a Suspense boundary whose fallback is the grid, so a
 * static prerender of this shell never trips the `useSearchParams`
 * client-rendering bailout.
 *
 * Exception: while a card cover is morphing into a detail page, the
 * series-detail skeleton shows instead, so the cover has its landing slot
 * (see `MorphAwareLoading`).
 */
export default function LibraryLoading() {
  const grid = <LibraryGridSkeleton />;
  return (
    <MorphAwareLoading detail={<SeriesDetailLoading />}>
      <LoadingWatchdog />
      <Suspense fallback={grid}>
        <LibraryQuerySwitch
          grid={grid}
          home={
            <PageRailsSkeleton
              rails={3}
              cardSizeKey={CARD_SIZE_KEYS.home}
              label="Loading home…"
            />
          }
        />
      </Suspense>
    </MorphAwareLoading>
  );
}

/** `/?library=…`: page heading + toolbar row over the series cover grid. */
function LibraryGridSkeleton() {
  return (
    <div>
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

      <RouteCardGrid aria-hidden className="grid gap-4">
        {Array.from({ length: 24 }, (_, i) => (
          <SeriesCardSkeleton key={i} />
        ))}
      </RouteCardGrid>
      <span className="sr-only">Loading…</span>
    </div>
  );
}
