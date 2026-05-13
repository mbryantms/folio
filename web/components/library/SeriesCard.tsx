"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";

import { Cover } from "@/components/Cover";
import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverLongPressActions } from "@/components/CoverLongPressActions";
import { useCoverMenuCollectionActions } from "@/components/collections/useCoverMenuCollectionActions";
import { SeriesPlayOverlay } from "@/components/QuickReadOverlay";
import { Badge } from "@/components/ui/badge";
import { jsonFetch } from "@/lib/api/queries";
import { useUpsertSeriesProgress } from "@/lib/api/mutations";
import type { SeriesResumeView, SeriesView } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { formatPublicationStatus } from "@/lib/format";
import { collectionStatus } from "@/lib/series-status";
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
  const router = useRouter();
  const upsertSeriesProgress = useUpsertSeriesProgress(series.id);
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "series",
    ref_id: series.id,
    label: series.name,
  });
  const menuActions: CoverMenuAction[] = [
    {
      label: "Mark all read",
      onSelect: () => upsertSeriesProgress.mutate({ finished: true }),
    },
    {
      label: "Mark all unread",
      onSelect: () => upsertSeriesProgress.mutate({ finished: false }),
    },
    ...collectionActions.actions,
    ...(extraActions ?? []),
  ];
  const longPress = useCoverLongPressActions({
    primary: {
      label: `Read ${series.name}`,
      onSelect: async () => {
        // Mirrors SeriesPlayOverlay — async resume lookup before routing.
        // Failures are quiet because the user can still drill into the
        // series detail page from the sheet's "Open" gesture or by
        // closing the sheet and short-tapping.
        try {
          const resume = await jsonFetch<SeriesResumeView>(
            `/series/${encodeURIComponent(series.slug)}/resume`,
          );
          if (!resume.issue_slug) return;
          router.push(
            `/read/${encodeURIComponent(resume.series_slug)}/${encodeURIComponent(resume.issue_slug)}`,
          );
        } catch {
          // ignore — match SeriesPlayOverlay's quiet-fail behavior
        }
      },
    },
    actions: menuActions,
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
        <div className="relative" {...longPress.wrapperProps}>
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
            actions={menuActions}
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
      {longPress.sheet}
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
