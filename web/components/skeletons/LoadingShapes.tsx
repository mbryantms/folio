import type * as React from "react";

import { CoverSkeleton } from "@/components/CoverSkeleton";
import { IssueCardSkeleton } from "@/components/library/IssueCard";
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
 *  (pass `label=""` for a shape nested inside another labelled region). */
export function LoadingRegion({
  label,
  className,
  style,
  children,
}: {
  label: string;
  className?: string;
  style?: React.CSSProperties;
  children: React.ReactNode;
}) {
  return (
    <div className={className} style={style} aria-busy="true">
      <div aria-hidden className="contents">
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

/** Horizontal cover rails, each with a label + count header. */
export function RailsSkeleton({
  rails = 2,
  label = "Loading…",
}: {
  rails?: number;
  label?: string;
}) {
  return (
    <LoadingRegion
      label={label}
      className="flex flex-col"
      style={{ gap: "var(--density-rail-gap, 2.5rem)" }}
    >
      {Array.from({ length: rails }, (_, rail) => (
        <div key={rail} className="space-y-3">
          <div className="flex items-center gap-2">
            <Skeleton className={cn("h-5", rail % 2 ? "w-32" : "w-24")} />
            <Skeleton className="h-3 w-12" />
          </div>
          <div className="flex gap-4 overflow-hidden">
            {Array.from({ length: 8 }, (_, i) => (
              <div key={i} className="w-40 shrink-0 space-y-2">
                <CoverSkeleton />
                <Skeleton className={cn("h-4", lineWidth(i + rail))} />
                <Skeleton className="h-3 w-1/2" />
              </div>
            ))}
          </div>
        </div>
      ))}
    </LoadingRegion>
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

/** `PageHeader` stand-in: optional breadcrumb, title, description. */
export function PageHeaderSkeleton({
  breadcrumb = false,
}: {
  breadcrumb?: boolean;
}) {
  return (
    <div
      aria-hidden
      className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4"
    >
      <div className="min-w-0 space-y-2">
        {breadcrumb ? <Skeleton className="h-3 w-24" /> : null}
        <Skeleton className="h-7 w-56 max-w-full" />
        <Skeleton className="h-4 w-80 max-w-full" />
      </div>
    </div>
  );
}

/** Entity landing page (`/arcs/<slug>`, `/characters/<slug>`, …): header,
 *  profile blurb, tab strip and the issue grid. */
export function EntityDetailSkeleton({
  label = "Loading…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <PageHeaderSkeleton breadcrumb />
      <div className="max-w-prose space-y-2">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-2/3" />
      </div>
      <TabStripSkeleton tabs={2} />
      <ul
        className="grid gap-4"
        style={{
          gridTemplateColumns: "repeat(auto-fill, minmax(160px, 1fr))",
        }}
      >
        {Array.from({ length: 12 }, (_, i) => (
          <li key={i}>
            <IssueCardSkeleton />
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}
