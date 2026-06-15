"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { memo, useState } from "react";

import { Cover } from "@/components/Cover";
import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverLongPressActions } from "@/components/CoverLongPressActions";
import { useCoverMenuCollectionActions } from "@/components/collections/useCoverMenuCollectionActions";
import { SeriesPlayOverlay } from "@/components/QuickReadOverlay";
import { BulkMarkReadDialog } from "@/components/library/BulkMarkReadDialog";
import { SelectionCheckbox } from "@/components/library/SelectionCheckbox";
import { useCoverCollectionDot } from "@/components/library/use-cover-collection-dot";
import { Badge } from "@/components/ui/badge";
import { jsonFetch } from "@/lib/api/queries";
import { useUpsertSeriesProgress } from "@/lib/api/mutations";
import type { SeriesResumeView, SeriesView } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { statusToneDot, statusToneSolid } from "@/lib/ui/status-tone";
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

function SeriesCardImpl({
  series,
  size = "md",
  href,
  className,
  extraActions,
  selectMode,
  onEnterSelectMode,
}: {
  series: SeriesView;
  size?: Size;
  href?: string;
  className?: string;
  /** Appended to the cover-menu's default actions (mark read/unread,
   *  add-to-collection). Use for surface-specific affordances like
   *  "Remove from this collection" on the collection detail page. */
  extraActions?: CoverMenuAction[];
  /** Multi-select mode toggle. Same shape as `<IssueCard>`'s
   *  `selectMode`: when set, the card click toggles selection
   *  instead of navigating; the long-press sheet stays dormant; a
   *  `<SelectionCheckbox>` overlay renders.
   *
   *  Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md`
   *  (M1 introduced the prop on IssueCard; M4 brings it to
   *  SeriesCard for collection-detail bulk-remove). */
  selectMode?: {
    isActive: boolean;
    isSelected: boolean;
    onToggle: (ev?: React.MouseEvent) => void;
  };
  /** Optional callback for the long-press sheet's "Select" entry.
   *  When set, mobile users get a second entry-point into select
   *  mode (besides the page-chrome "Select" button). Plan: M6. */
  onEnterSelectMode?: (id: string) => void;
}) {
  const c = sizeClasses[size];
  const status = formatPublicationStatus(series.status);
  const link = href ?? seriesUrl(series);
  const issueCount = series.issue_count ?? series.total_issues ?? null;
  const router = useRouter();
  const upsertSeriesProgress = useUpsertSeriesProgress(series.id);
  const [markAllReadOpen, setMarkAllReadOpen] = useState(false);
  const collectionActions = useCoverMenuCollectionActions({
    entry_kind: "series",
    ref_id: series.id,
    label: series.name,
  });
  const menuActions: CoverMenuAction[] = [
    {
      label: "Mark all read",
      // Whole-series mark-all is almost always cataloging — prompt
      // so the user picks whether it counts toward reading activity.
      onSelect: () => setMarkAllReadOpen(true),
    },
    {
      label: "Mark all unread",
      onSelect: () => upsertSeriesProgress.mutate({ finished: false }),
    },
    ...collectionActions.actions,
    ...(extraActions ?? []),
  ];
  // Prepend "Select" to the long-press sheet when the parent
  // surface supports multi-select. Mirrors the IssueCard pattern;
  // gated by both `onEnterSelectMode` being set AND not already
  // being in select mode (the long-press handler is suppressed in
  // that case anyway).
  const sheetActions: CoverMenuAction[] =
    onEnterSelectMode && !selectMode?.isActive
      ? [
          {
            label: "Select",
            onSelect: () => onEnterSelectMode(series.id),
          },
          ...menuActions,
        ]
      : menuActions;
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
    actions: sheetActions,
    label: series.name,
  });
  // The "Add to Collection…" dialog must render as a *sibling* of the
  // <Link>, not a child — React synthetic events bubble through the
  // React tree even across portals, so a click inside the dialog
  // would otherwise propagate to the Link's onClick and trigger
  // navigation. Hoisting the dialog out fixes the "modal flashes then
  // routes to the issue page" bug seen on every cover-menu card.
  //
  // When `selectMode.isActive` is true the outer becomes a `<button>`
  // and the long-press wrapper props stay dormant — taps toggle
  // selection instead of opening the existing actions sheet.
  const inSelectMode = selectMode?.isActive ?? false;
  const cardOuterProps = inSelectMode
    ? {
        type: "button" as const,
        onClick: (ev: React.MouseEvent) => {
          ev.preventDefault();
          selectMode?.onToggle(ev);
        },
        "aria-pressed": selectMode?.isSelected ?? false,
      }
    : null;
  const coverWrapperProps = inSelectMode ? {} : longPress.wrapperProps;
  const cardClassName = cn(
    "group hover:bg-accent/40 focus-visible:ring-ring flex shrink-0 flex-col gap-2 rounded-md p-1 transition-colors focus-visible:ring-2 focus-visible:outline-none",
    c.wrap,
    inSelectMode && "text-left w-full cursor-pointer",
    inSelectMode &&
      selectMode?.isSelected &&
      "bg-primary/5 ring-2 ring-primary/40",
    className,
  );
  const innerCard = (
    <>
      <div className="relative" {...coverWrapperProps}>
        {selectMode && (
          <SelectionCheckbox
            isSelected={selectMode.isSelected}
            selectMode={selectMode.isActive}
            onToggle={selectMode.onToggle}
            label={series.name}
          />
        )}
        <Cover
          src={series.cover_url}
          alt={series.name}
          fallback={series.publisher ?? series.name}
          className="w-full transition group-hover:brightness-110"
        />
        {/* Top-right indicator stack: publication status + the
         *  metadata-needs chip. Both live here (not top-left) so the
         *  kebab affordance owns the canonical top-left across all card
         *  types — and so the *interactive* meta chip stays clickable on
         *  desktop, where hovering the card reveals the kebab over the
         *  top-left corner. */}
        <div className="absolute top-2 right-2 z-10 flex flex-col items-end gap-1">
          {status && status !== "Active" && (
            <Badge
              variant="secondary"
              className="bg-background/80 backdrop-blur"
            >
              {status}
            </Badge>
          )}
          <MetaNeedsBadge series={series} interactive={!inSelectMode} />
        </div>
        <CollectionDot series={series} />
        {!inSelectMode && (
          <>
            <CoverMenuButton
              label={`Actions for ${series.name}`}
              actions={menuActions}
            />
            <SeriesPlayOverlay
              seriesSlug={series.slug}
              seriesName={series.name}
            />
          </>
        )}
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
    </>
  );
  return (
    <>
      {cardOuterProps ? (
        <button className={cardClassName} {...cardOuterProps}>
          {innerCard}
        </button>
      ) : (
        <Link href={link} className={cardClassName}>
          {innerCard}
        </Link>
      )}
      {collectionActions.dialog}
      {!inSelectMode && longPress.sheet}
      <BulkMarkReadDialog
        open={markAllReadOpen}
        onOpenChange={setMarkAllReadOpen}
        count={issueCount ?? 0}
        title={`Mark every issue in ${series.name} as read?`}
        onConfirm={(backfill) =>
          upsertSeriesProgress.mutate(
            { finished: true, backfill },
            { onSuccess: () => setMarkAllReadOpen(false) },
          )
        }
        isPending={upsertSeriesProgress.isPending}
      />
    </>
  );
}

/** Memoized: grid/rail surfaces mount hundreds of these at once and
 *  their props are referentially stable (cache rows + literals), so
 *  parent state churn — search keystrokes, selection toggles,
 *  sentinel observer resets — no longer reconciles every card. */
export const SeriesCard = memo(SeriesCardImpl);

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

/** Small green/amber dot in the cover's bottom-left corner. Lives opposite
 *  the play overlay so the 32px hover button doesn't cover it. The
 *  symmetric bottom-left slot on issue cards hosts the "finished"
 *  check badge — same idiom, different axis (ownership vs. read state).
 *
 *  Suppressed entirely when the user has disabled
 *  `useCoverCollectionDot` from the CardSizeOptions popover — gives
 *  readers who want pristine covers an opt-out. */
function CollectionDot({ series }: { series: SeriesView }) {
  const dotPref = useCoverCollectionDot();
  const state = collectionStatus(series);
  if (!state || !dotPref.enabled) return null;
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
        "absolute bottom-2 left-2 h-2.5 w-2.5 rounded-full ring-1 ring-black/10 dark:ring-white/10",
        state === "complete"
          ? statusToneDot("success")
          : statusToneDot("warning"),
      )}
    />
  );
}

/** Small amber "metadata" chip shown only when the series' metadata is so
 *  sparse it likely needs pulling (`metadata_completeness_tier ===
 *  "needs_metadata"`). Positioned by the parent's top-right indicator stack
 *  (kept out of the top-left kebab corner). Shares the `useCoverCollectionDot`
 *  opt-out so pristine-cover readers hide both cover overlays at once.
 *
 *  When `interactive`, the chip "links to the fix" (B4): clicking it jumps to
 *  the series page with `?match=1`, which auto-opens the metadata match
 *  dialog (see `SeriesSettingsMenu`). Rendered as `<span role="button">` for
 *  the same reason as `QuickReadOverlay` — a `<button>`/`<a>` nested in the
 *  card's parent `<Link>` would be invalid HTML. While selecting, the chip
 *  stays a passive badge so the card's selection toggle owns the click. */
export function MetaNeedsBadge({
  series,
  interactive,
}: {
  series: SeriesView;
  interactive: boolean;
}) {
  const dotPref = useCoverCollectionDot();
  const router = useRouter();
  if (!dotPref.enabled) return null;
  if (series.metadata_completeness_tier !== "needs_metadata") return null;
  const badgeClass = cn(
    "inline-flex items-center rounded-md px-1.5 py-0.5 text-[10px] font-medium ring-1 ring-black/10 dark:ring-white/10",
    statusToneSolid("warning"),
  );

  if (!interactive) {
    const label = "Metadata likely incomplete";
    return (
      <span title={label} aria-label={label} className={badgeClass}>
        meta
      </span>
    );
  }

  const label = "Find metadata — likely incomplete";
  const activate = () => router.push(`${seriesUrl(series)}?match=1`);
  return (
    <span
      role="button"
      tabIndex={0}
      title={label}
      aria-label={label}
      onClick={(e) => {
        // The chip sits inside the card's parent <Link>; stop the parent
        // navigation so only the match deep-link fires.
        e.preventDefault();
        e.stopPropagation();
        activate();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          e.stopPropagation();
          activate();
        }
      }}
      className={cn(
        badgeClass,
        "cursor-pointer transition hover:brightness-110 focus-visible:ring-2 focus-visible:ring-white/40 focus-visible:outline-none",
      )}
    >
      meta
    </span>
  );
}
