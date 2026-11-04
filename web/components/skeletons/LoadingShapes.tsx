import type * as React from "react";

import { CoverSkeleton } from "@/components/CoverSkeleton";
import { IssueCardSkeleton } from "@/components/library/IssueCard";
import { ProgressIssueCardSkeleton } from "@/components/library/ProgressIssueCard";
import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * Shaped loading stand-ins shared by the in-page loading branches and the
 * route `loading.tsx` files (folio-flair "cover-shaped skeletons", pre-merge
 * polish). A route skeleton hands off to the same shape once the client
 * component mounts, so the layout holds until content lands instead of
 * collapsing to a line of "Loading…" text in between.
 *
 * Every shape is a server-safe, static tree under the shared shimmer
 * (`.skeleton-shimmer`, which the reduced-motion rules already freeze).
 * The visual boxes are hidden from assistive tech; a single sr-only label
 * announces what is loading.
 */

/** Varied line widths so stacked rows don't read as a barcode. */
const LINE_WIDTHS = ["w-2/3", "w-1/2", "w-3/4", "w-2/5", "w-3/5"] as const;

export function lineWidth(i: number): string {
  return LINE_WIDTHS[i % LINE_WIDTHS.length] ?? "w-1/2";
}

/** Wrapper: `aria-busy` region whose boxes are hidden, plus a spoken label
 *  (pass `label=""` for a shape nested inside another labelled region).
 *
 *  `className`/`style` land on the element that directly holds `children`,
 *  so layout utilities (`space-y-*`, `grid`, `flex gap-*`) space the boxes
 *  themselves. `rootClassName` styles the outer busy region, for callers
 *  whose region is itself a flex item (`flex-1 min-h-0`). */
export function LoadingRegion({
  label,
  className,
  rootClassName,
  style,
  children,
}: {
  label: string;
  className?: string;
  rootClassName?: string;
  style?: React.CSSProperties;
  children: React.ReactNode;
}) {
  return (
    <div className={rootClassName} aria-busy="true">
      <div aria-hidden className={className} style={style}>
        {children}
      </div>
      {label ? (
        <span className="sr-only" role="status">
          {label}
        </span>
      ) : null}
    </div>
  );
}

/** A grid of issue-card skeletons (cover + two caption lines). */
export function CoverGridSkeleton({
  count = 12,
  style,
  className,
  label = "Loading…",
}: {
  count?: number;
  /** Grid template, usually the page's own `gridStyle` so columns match. */
  style?: React.CSSProperties;
  className?: string;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <ul
        className={cn("grid gap-4", className)}
        style={
          style ?? {
            gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
          }
        }
      >
        {Array.from({ length: count }, (_, i) => (
          <li key={i}>
            <IssueCardSkeleton />
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** Horizontal cover rails shaped like `SavedViewRail`: an icon + `text-lg`
 *  title header, then a `HorizontalScrollRail` track of `cardSize`-wide
 *  cards ending on the "View all" tail. `card` picks the card skeleton:
 *  `"progress"` (Continue reading's three-line caption) for the first
 *  rail then issue cards, or series cards throughout. */
export function RailsSkeleton({
  rails = 2,
  card = "issue",
  cardSize = 160,
  label = "Loading…",
}: {
  rails?: number;
  card?: "issue" | "series";
  /** Card width in px (the page's card-size preference; 160 default). */
  cardSize?: number;
  label?: string;
}) {
  const width = { width: `${cardSize}px` };
  return (
    <LoadingRegion
      label={label}
      className="flex flex-col"
      style={{ gap: "var(--density-rail-gap, 2.5rem)" }}
    >
      {Array.from({ length: rails }, (_, rail) => (
        <div
          key={rail}
          className="flex flex-col"
          style={{ gap: "var(--density-rail-inner-gap, 0.75rem)" }}
        >
          <div className="flex min-w-0 items-center gap-1.5">
            <Skeleton className="size-[18px] shrink-0 rounded-md" />
            <TextLineSkeleton
              lineClassName="h-7"
              className={cn("h-4", rail % 2 ? "w-40" : "w-32")}
            />
          </div>
          <RailTrackSkeleton>
            {Array.from({ length: 8 }, (_, i) => (
              <div key={i} style={width} className="shrink-0">
                {card === "series" ? (
                  <SeriesCardSkeleton />
                ) : rail === 0 ? (
                  <ProgressIssueCardSkeleton />
                ) : (
                  <IssueCardSkeleton />
                )}
              </div>
            ))}
            <div style={width} className="flex shrink-0 items-center">
              <Skeleton className="h-8 w-28 rounded-md" />
            </div>
          </RailTrackSkeleton>
        </div>
      ))}
    </LoadingRegion>
  );
}

/** A rails page (`PageRails`: Home and `/pages/<slug>`). From `sm` up:
 *  the heading (title + description) in flow, then the right-aligned
 *  toolbar row (View options + `⋯`). Below `sm`: the sticky phone bar
 *  (title · View options · `⋯`), plus the description line when the page
 *  has one. Then the rails. */
export function PageRailsSkeleton({
  rails = 3,
  card = "issue",
  phoneDescription = false,
  label = "Loading…",
}: {
  rails?: number;
  card?: "issue" | "series";
  /** Custom pages with a description repeat it under the phone bar. */
  phoneDescription?: boolean;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <div className="flex max-sm:hidden">
        <div className="min-w-0 flex-1">
          <div className="flex h-8 items-center">
            <Skeleton className="h-6 w-32" />
          </div>
          <TextLineSkeleton lineClassName="mt-1 h-5" className="w-64" />
        </div>
      </div>
      <div className="-mx-4 mb-(--density-page-pad-y) flex flex-wrap items-center justify-end gap-2 px-4 py-2 max-sm:hidden md:-mx-8 md:px-8">
        <Skeleton className="h-9 w-[38px] rounded-md" />
        <Skeleton className="h-9 w-9 rounded-md" />
      </div>
      <div
        className={cn(
          "border-border -mx-4 -mt-6 border-b px-4 py-2 sm:hidden",
          phoneDescription ? "mb-3" : "mb-(--density-page-pad-y)",
        )}
      >
        <div className="flex items-center gap-2">
          <div className="flex h-7 items-center">
            <Skeleton className="h-5 w-28" />
          </div>
          <div className="ml-auto flex shrink-0 items-center gap-1">
            <Skeleton className="h-9 w-9 rounded-md" />
            <Skeleton className="h-9 w-9 rounded-md" />
          </div>
        </div>
      </div>
      {phoneDescription ? (
        <TextLineSkeleton
          lineClassName="mb-(--density-page-pad-y) h-4 sm:hidden"
          className="h-3 w-3/4"
        />
      ) : null}
      <RailsSkeleton rails={rails} card={card} label="" />
    </LoadingRegion>
  );
}

/** `HorizontalScrollRail` track stand-in: the same flow-root wrapper,
 *  `-my-1` scroller with the density bottom padding, and `flex gap-3 p-1`
 *  track, clipped instead of scrolling. Children are the fixed-width
 *  cells. */
export function RailTrackSkeleton({
  children,
  trackClassName,
}: {
  children: React.ReactNode;
  trackClassName?: string;
}) {
  return (
    <div className="relative flow-root">
      <div
        className="relative -my-1 overflow-hidden"
        style={{ paddingBottom: "var(--density-rail-pb)" }}
      >
        <div className={cn("flex gap-3 p-1", trackClassName)}>{children}</div>
      </div>
    </div>
  );
}

/** A bordered, divided list of rows: optional cover thumb, two lines and a
 *  trailing pill. Settings lists, appearance lists, resolution queues. */
export function RowsSkeleton({
  rows = 6,
  cover = false,
  trailing = true,
  className,
  label = "Loading…",
}: {
  rows?: number;
  cover?: boolean;
  trailing?: boolean;
  className?: string;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <ul
        className={cn(
          "border-border/60 divide-border/60 divide-y overflow-hidden rounded-md border",
          className,
        )}
      >
        {Array.from({ length: rows }, (_, i) => (
          <li key={i} className="flex items-center gap-3 px-3 py-2.5">
            {cover ? <CoverSkeleton className="w-8 shrink-0" /> : null}
            <div className="min-w-0 flex-1 space-y-1.5">
              <Skeleton className={cn("h-4", lineWidth(i))} />
              {cover ? <Skeleton className="h-3 w-24" /> : null}
            </div>
            {trailing ? (
              <Skeleton className="h-5 w-16 shrink-0 rounded-full" />
            ) : null}
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** Rows laid out on a caller-supplied grid template (table bodies). */
export function TableRowsSkeleton({
  rows = 8,
  columns,
  gridClassName,
  rowClassName,
  label = "Loading…",
}: {
  rows?: number;
  columns: number;
  /** The table's own `grid-cols-[…]` so cells line up with the header. */
  gridClassName: string;
  rowClassName?: string;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      {Array.from({ length: rows }, (_, r) => (
        <div
          key={r}
          className={cn(
            "border-border grid items-center gap-2 border-b px-3 py-3",
            gridClassName,
            rowClassName,
          )}
        >
          {Array.from({ length: columns }, (_, c) => (
            <Skeleton
              key={c}
              className={cn("h-3.5", c === 1 ? lineWidth(r) : "w-3/4")}
            />
          ))}
        </div>
      ))}
    </LoadingRegion>
  );
}

/** A grid of plain card tiles (stat strips, metadata cards, view boards).
 *  `className` carries the grid columns — keep a `grid-cols-1` base. */
export function TilesSkeleton({
  count = 4,
  className = "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4",
  tileClassName = "h-24",
  label = "Loading…",
}: {
  count?: number;
  className?: string;
  tileClassName?: string;
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className={className}>
      {Array.from({ length: count }, (_, i) => (
        <Skeleton key={i} className={cn("w-full rounded-lg", tileClassName)} />
      ))}
    </LoadingRegion>
  );
}

/** A row of tab triggers. */
export function TabStripSkeleton({ tabs = 3 }: { tabs?: number }) {
  return (
    <div aria-hidden className="bg-muted/50 flex w-fit gap-1 rounded-lg p-1">
      {Array.from({ length: tabs }, (_, i) => (
        <Skeleton
          key={i}
          className={cn("h-7 rounded-md", i % 2 ? "w-28" : "w-24")}
        />
      ))}
    </div>
  );
}

/** The `line` variant `TabsList` (`components/folio/tabs`, the default):
 *  a full-width 36px strip on a hairline, label-sized marks per trigger. */
export function LineTabStripSkeleton({
  widths = ["w-16", "w-14", "w-20"],
}: {
  widths?: readonly string[];
}) {
  return (
    <div
      aria-hidden
      className="border-border flex h-9 w-full items-center gap-1 overflow-hidden border-b p-[3px]"
    >
      {widths.map((w, i) => (
        <div key={i} className="flex h-full shrink-0 items-center px-2">
          <Skeleton className={cn("h-3.5", w)} />
        </div>
      ))}
    </div>
  );
}

/** One text line: a box the height of the real line box with a shorter
 *  bar centred in it, so stacked lines take the same vertical room as the
 *  text they stand in for (`lineClassName` is the line-height class). */
export function TextLineSkeleton({
  lineClassName = "h-5",
  className,
}: {
  lineClassName?: string;
  className?: string;
}) {
  return (
    <div className={cn("flex items-center", lineClassName)}>
      <Skeleton className={cn("h-3.5", className)} />
    </div>
  );
}

/** `PageHeader` stand-in (`components/admin/PageHeader`): optional
 *  breadcrumb, title, description and right-hand actions, on the same
 *  line boxes, border and bottom margin. The margin is `mb-6` like the
 *  real header, so it also works outside a `space-y-6` stack. */
export function PageHeaderSkeleton({
  breadcrumb = false,
  description = true,
  phoneDescription = "visible",
  titleClassName = "w-56",
  descriptionClassName = "w-80",
  actions,
  className,
}: {
  breadcrumb?: boolean;
  description?: boolean;
  phoneDescription?: "hidden" | "visible";
  titleClassName?: string;
  descriptionClassName?: string;
  /** Skeleton boxes for the actions slot (sized like the real buttons). */
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      aria-hidden
      className={cn(
        "border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4 max-sm:mb-4 max-sm:pb-3",
        className,
      )}
    >
      <div className="min-w-0">
        {breadcrumb ? (
          <TextLineSkeleton lineClassName="mb-1.5 h-4" className="h-3 w-32" />
        ) : null}
        <div className="flex h-8 items-center max-sm:h-7">
          <Skeleton
            className={cn("h-6 max-w-full max-sm:h-5", titleClassName)}
          />
        </div>
        {description ? (
          <TextLineSkeleton
            lineClassName={cn(
              "mt-1 h-5 max-sm:h-4",
              phoneDescription === "hidden" && "max-sm:hidden",
            )}
            className={cn("h-3.5 max-w-full", descriptionClassName)}
          />
        ) : null}
      </div>
      {actions ? (
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      ) : null}
    </div>
  );
}

/** The entity page header (`EntityDetail` draws PageHeader's markup by
 *  hand): breadcrumb, name, then the "N issues · N series · …" facts line. */
function EntityHeaderSkeleton({ facts = "w-72" }: { facts?: string }) {
  return (
    <PageHeaderSkeleton
      breadcrumb
      titleClassName="w-64"
      descriptionClassName={facts}
    />
  );
}

/** The provider profile card (`EntityProfile`, enriched shape): image on
 *  the left, the fact grid, then the "Filled from …" footer with Refresh. */
function EntityProfileSkeleton() {
  return (
    <div className="bg-card border-border rounded-lg border p-4 sm:p-6">
      <div className="flex flex-col gap-5 sm:flex-row">
        <Skeleton className="h-64 w-44 shrink-0 self-start rounded-md" />
        <div className="min-w-0 flex-1 space-y-4">
          <div className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-2 lg:grid-cols-3">
            {Array.from({ length: 6 }, (_, i) => (
              <div key={i}>
                <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-20" />
                <TextLineSkeleton
                  lineClassName="mt-0.5 h-5"
                  className={lineWidth(i)}
                />
              </div>
            ))}
          </div>
          <div className="space-y-1.5">
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-24" />
            <div className="flex flex-wrap gap-1.5">
              {["w-20", "w-28", "w-16", "w-24"].map((w, i) => (
                <Skeleton key={i} className={cn("h-5", w)} />
              ))}
            </div>
          </div>
        </div>
      </div>
      <div className="border-border mt-5 flex flex-wrap items-center gap-x-4 gap-y-2 border-t pt-4">
        <TextLineSkeleton lineClassName="h-4" className="h-3 w-48" />
        <TextLineSkeleton lineClassName="h-4" className="h-3 w-64" />
        <Skeleton className="ml-auto h-7 w-20" />
      </div>
    </div>
  );
}

/** "In your library" (`EntityInsights`): heading, four stat tiles, the
 *  earliest-appearances rail and the decade chart | characters row. Also
 *  the component's own loading state, so the route → client handoff
 *  holds still. */
export function EntityInsightsSkeleton() {
  return (
    <div className="space-y-4">
      <TextLineSkeleton lineClassName="h-7" className="h-5 w-36" />
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {Array.from({ length: 4 }, (_, i) => (
          <div
            key={i}
            className="bg-card border-border rounded-lg border px-4 py-3"
          >
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-14" />
            <TextLineSkeleton lineClassName="mt-1 h-8" className="h-6 w-16" />
            <TextLineSkeleton
              lineClassName="mt-0.5 h-4"
              className={cn("h-2.5", lineWidth(i))}
            />
          </div>
        ))}
      </div>
      <div className="bg-card border-border space-y-3 rounded-lg border p-4">
        <TextLineSkeleton className="h-3.5 w-48" />
        <div className="flex gap-3 overflow-hidden pb-1">
          {Array.from({ length: 10 }, (_, i) => (
            <IssueCardSkeleton key={i} className="w-[120px] shrink-0" />
          ))}
        </div>
      </div>
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div className="bg-card border-border space-y-3 rounded-lg border p-4">
          <TextLineSkeleton className="h-3.5 w-32" />
          <div className="flex h-36 items-end gap-2">
            {[0.15, 0.4, 0.75, 1, 0.55, 0.3].map((h, i) => (
              <div
                key={i}
                className="flex min-w-0 flex-1 flex-col items-center gap-1"
              >
                <Skeleton className="h-3 w-6" />
                <Skeleton
                  className="w-full rounded-none rounded-t-sm"
                  style={{ height: `${Math.round(h * 100)}px` }}
                />
                <Skeleton className="h-3 w-5" />
              </div>
            ))}
          </div>
        </div>
        <div className="bg-card border-border space-y-3 rounded-lg border p-4">
          <TextLineSkeleton className="h-3.5 w-24" />
          <div className="flex flex-wrap gap-1.5">
            {Array.from({ length: 14 }, (_, i) => (
              <Skeleton
                key={i}
                className={cn(
                  "h-7 rounded-full",
                  ["w-24", "w-32", "w-20", "w-28"][i % 4],
                )}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

/** Entity landing page (`/arcs/<slug>`, `/characters/<slug>`, `/teams/…`,
 *  `/locations/…`), shaped like `EntityDetail`: header, the profile card,
 *  then "In your library" down through the decade/characters row (the
 *  Issues/Series tabs sit below the fold). */
export function EntityDetailSkeleton({
  label = "Loading…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <EntityHeaderSkeleton />
      <EntityProfileSkeleton />
      <EntityInsightsSkeleton />
    </LoadingRegion>
  );
}

/** `/publishers/<slug>`: publishers have no provider profile or tabs —
 *  the header ("N series") sits straight over the series cover grid. */
export function PublisherDetailSkeleton({
  label = "Loading…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <EntityHeaderSkeleton facts="w-20" />
      <ul
        className="grid gap-4"
        style={{
          gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
        }}
      >
        {Array.from({ length: 12 }, (_, i) => (
          <li key={i}>
            <SeriesCardSkeleton />
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}
