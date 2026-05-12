"use client";

import Link from "next/link";

import { Cover } from "@/components/Cover";
import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverMenuCollectionActions } from "@/components/collections/useCoverMenuCollectionActions";
import { SeriesPlayOverlay } from "@/components/QuickReadOverlay";
import { Badge } from "@/components/ui/badge";
import { useUpsertSeriesProgress } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";
import { formatPublicationStatus } from "@/lib/format";
import { collectionStatus } from "@/lib/series-status";
import type { SeriesView } from "@/lib/api/types";
import { seriesUrl } from "@/lib/urls";

type Size = "sm" | "md";

const sizeClasses: Record<Size, { wrap: string; title: string; meta: string }> =
  {
    sm: {
      wrap: "w-36 sm:w-40",
      title: "text-sm",
      meta: "text-xs text-muted-foreground",
    },
    md: {
      wrap: "w-full",
      title: "text-base",
      meta: "text-xs text-muted-foreground",
    },
  };

export function SeriesCard({
  series,
  size = "md",
  href,
  className,
  extraActions,
}: {
  series: SeriesView;
  size?: Size;
  href?: string;
  className?: string;
  /** Appended to the cover-menu's default actions (mark read/unread,
   *  add-to-collection). Use for surface-specific affordances like
   *  "Remove from this collection" on the collection detail page. */
  extraActions?: CoverMenuAction[];
}) {
  const c = sizeClasses[size];
  const status = formatPublicationStatus(series.status);
  const link = href ?? seriesUrl(series);
  const issueCount = series.issue_count ?? series.total_issues ?? null;
  const upsertSeriesProgress = useUpsertSeriesProgress(series.id);
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "series",
    ref_id: series.id,
    label: series.name,
  });
  // The "Add to Collection…" dialog must render as a *sibling* of the
  // <Link>, not a child — React synthetic events bubble through the
  // React tree even across portals, so a click inside the dialog
  // would otherwise propagate to the Link's onClick and trigger
  // navigation. Hoisting the dialog out fixes the "modal flashes then
  // routes to the issue page" bug seen on every cover-menu card.
  return (
    <>
      <Link
        href={link}
        className={cn(
          "group hover:bg-accent/40 focus-visible:ring-ring flex shrink-0 flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
          c.wrap,
          className,
        )}
      >
        <div className="relative">
          <Cover
            src={series.cover_url}
            alt={series.name}
            fallback={series.publisher ?? series.name}
            className="w-full transition group-hover:brightness-110"
          />
          {/* Status badge moved to top-right so the kebab affordance can
           *  live at the canonical top-left across all card types. */}
          {status && status !== "Active" && (
            <Badge
              variant="secondary"
              className="bg-background/80 absolute top-2 right-2 backdrop-blur"
            >
              {status}
            </Badge>
          )}
          <CollectionDot series={series} />
          <CoverMenuButton
            label={`Actions for ${series.name}`}
            actions={[
              {
                label: "Mark all read",
                onSelect: () => upsertSeriesProgress.mutate({ finished: true }),
              },
              {
                label: "Mark all unread",
                onSelect: () =>
                  upsertSeriesProgress.mutate({ finished: false }),
              },
              ...collectionActions.actions,
              ...(extraActions ?? []),
            ]}
          />
          <SeriesPlayOverlay
            seriesSlug={series.slug}
            seriesName={series.name}
          />
        </div>
        <div className="min-w-0 px-1">
          <div
            className={cn("truncate font-medium", c.title)}
            title={series.name}
          >
            {series.name}
          </div>
          <div className={c.meta}>
            {[
              series.year,
              issueCount != null
                ? `${issueCount} issue${issueCount === 1 ? "" : "s"}`
                : null,
            ]
              .filter(Boolean)
              .join(" • ") || " "}
          </div>
        </div>
      </Link>
      {collectionActions.dialog}
    </>
  );
}

export function SeriesCardSkeleton({ size = "md" }: { size?: Size }) {
  const c = sizeClasses[size];
  return (
    <div className={cn("flex shrink-0 flex-col gap-2 p-1", c.wrap)}>
      <div className="bg-muted aspect-[2/3] w-full animate-pulse rounded-md" />
      <div className="space-y-1.5 px-1">
        <div className="bg-muted h-3 w-3/4 animate-pulse rounded" />
        <div className="bg-muted h-2 w-1/2 animate-pulse rounded" />
      </div>
    </div>
  );
}

/** Small green/amber dot in the cover's bottom-right corner. Sits behind
 *  the play overlay on hover — both anchor to `right-2 bottom-2`, the
 *  32px play button visually covers the 10px dot when revealed. */
function CollectionDot({ series }: { series: SeriesView }) {
  const state = collectionStatus(series);
  if (!state) return null;
  const have = series.issue_count ?? 0;
  const total = series.total_issues ?? 0;
  const tooltip =
    state === "complete"
      ? `Complete: ${have} of ${total} issues`
      : `${have} of ${total} issues`;
  return (
    <span
      title={tooltip}
      aria-label={tooltip}
      className={cn(
        "absolute right-2 bottom-2 h-2.5 w-2.5 rounded-full ring-1 ring-black/10 dark:ring-white/10",
        state === "complete" ? "bg-emerald-500" : "bg-amber-500",
      )}
    />
  );
}
