import type * as React from "react";

import {
  LoadingRegion,
  TextLineSkeleton,
  lineWidth,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * Reading-log loading shapes, shared by each tab's route `loading.tsx`
 * (shown under the log layout's header + tabs during client navigation)
 * and the tab component's own loading branch, so the route → client
 * handoff holds still. Server-safe static trees; boxes are aria-hidden
 * under `LoadingRegion`.
 */

/** `ActivityRangeSelector` (desktop: six content-sized pills in an h-9
 *  bordered group; phones: the group fills the row). */
export function RangeSelectorSkeleton({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "border-input bg-background inline-flex h-9 max-w-full rounded-md border p-0.5 max-sm:flex max-sm:w-full",
        className,
      )}
    >
      {["w-10", "w-12", "w-12", "w-12", "w-10", "w-12"].map((w, i) => (
        <div
          key={i}
          className="flex items-center justify-center px-3 max-sm:min-w-0 max-sm:flex-1 max-sm:px-1.5"
        >
          <Skeleton className={cn("h-3 max-sm:w-6", w)} />
        </div>
      ))}
    </div>
  );
}

/** Overview's `Section` heading: `text-lg` title + `text-sm` line. */
function SectionHeadSkeleton({
  title = "w-24",
  description = "w-64",
}: {
  title?: string;
  description?: string;
}) {
  return (
    <div>
      <TextLineSkeleton lineClassName="h-7" className={cn("h-4", title)} />
      <TextLineSkeleton className={cn("max-w-full", description)} />
    </div>
  );
}

/** `/log` Overview: "N reading days" line over four stat tiles, the Year
 *  in review banner, the Activity heatmap card and the milestone grid. */
export function OverviewSkeleton({
  label = "Loading reading log…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-10">
      <div className="space-y-4">
        <TextLineSkeleton className="w-40" />
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {Array.from({ length: 4 }, (_, i) => (
            <div
              key={i}
              className="bg-card min-w-0 rounded-xl border p-4 sm:p-5"
            >
              <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-24" />
              <TextLineSkeleton
                lineClassName="mt-1.5 h-9"
                className="h-7 w-16"
              />
              <TextLineSkeleton
                lineClassName="mt-0.5 h-4"
                className={cn("h-2.5", i === 3 ? "w-32" : "w-20")}
              />
            </div>
          ))}
        </div>
      </div>
      {/* YearBanner: p-5/p-6 around three text lines, or the h-24 cover
          fan from `sm` up. */}
      <Skeleton className="h-28 w-full rounded-2xl sm:h-36" />
      <div className="space-y-4">
        <SectionHeadSkeleton title="w-20" description="w-56" />
        <div className="bg-card rounded-xl border p-5">
          <HeatmapSkeleton />
        </div>
      </div>
      <div className="space-y-4">
        <SectionHeadSkeleton title="w-28" description="w-96" />
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {Array.from({ length: 3 }, (_, i) => (
            <div
              key={i}
              className="bg-card flex items-center gap-3 rounded-xl border p-4"
            >
              <Skeleton className="size-11 shrink-0 rounded-full" />
              <div className="min-w-0 flex-1">
                <TextLineSkeleton className={lineWidth(i)} />
                <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-24" />
              </div>
            </div>
          ))}
        </div>
      </div>
    </LoadingRegion>
  );
}

/** `ReadingHeatmap`: the same container-sized cell (`--hm-cell`), so the
 *  block is as tall as the 7-row year grid + month row + legend. */
function HeatmapSkeleton() {
  return (
    <div className="[container-type:inline-size]">
      <div
        style={
          {
            "--hm-cell": "clamp(10px, calc((100cqi - 3rem) / 53 - 3px), 24px)",
          } as React.CSSProperties
        }
      >
        <div className="flex gap-2">
          <div className="w-7 shrink-0 pt-5">
            <Skeleton
              className="w-full"
              style={{ height: "calc(7 * var(--hm-cell) + 18px)" }}
            />
          </div>
          <div className="min-w-0 flex-1 pb-1">
            <div className="flex h-5 items-start">
              <Skeleton className="h-2.5 w-full" />
            </div>
            <Skeleton
              className="w-full"
              style={{ height: "calc(7 * var(--hm-cell) + 18px)" }}
            />
          </div>
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-28" />
          <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-24" />
        </div>
      </div>
    </div>
  );
}

/** `ActivityTimeline` body: day groups (uppercase label) of two-line
 *  session rows with the duration + pages on the right. */
export function SessionsTimelineSkeleton({
  groups = [8, 4],
  label = "Loading sessions…",
}: {
  groups?: readonly number[];
  label?: string;
}) {
  let n = 0;
  return (
    <LoadingRegion label={label} className="space-y-5">
      {groups.map((rows, g) => (
        <div key={g}>
          <TextLineSkeleton
            lineClassName="mb-1.5 h-4"
            className={cn("h-2.5", g === 0 ? "w-12" : "w-20")}
          />
          <ul className="divide-border divide-y">
            {Array.from({ length: rows }, () => {
              const i = n++;
              return (
                <li
                  key={i}
                  className="flex items-center justify-between gap-3 py-2"
                >
                  <div className="min-w-0 flex-1">
                    <TextLineSkeleton
                      className={cn("max-w-64", lineWidth(i))}
                    />
                    <TextLineSkeleton
                      lineClassName="h-4"
                      className="h-2.5 w-32"
                    />
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Skeleton className="h-3.5 w-10" />
                    <Skeleton className="h-2.5 w-6" />
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </LoadingRegion>
  );
}

/** `/log/sessions`: intro line over the bordered timeline card. */
export function SessionsPageSkeleton() {
  return (
    <div className="space-y-4">
      <TextLineSkeleton className="w-80 max-w-full" />
      <div className="bg-card rounded-xl border p-5">
        <SessionsTimelineSkeleton label="Loading reading sessions…" />
      </div>
    </div>
  );
}

/** Finished tab stat tiles (`Card` p-5: icon, `text-3xl` value, label). */
export function FinishedStatsSkeleton() {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      {["w-12", "w-16", "w-12", "w-10"].map((w, i) => (
        <div
          key={i}
          className="bg-card ring-foreground/10 flex flex-col gap-1.5 rounded-xl p-5 shadow-xs ring-1"
        >
          <Skeleton className="size-4 rounded" />
          <TextLineSkeleton
            lineClassName="h-[2.35rem]"
            className={cn("h-7", w)}
          />
          <TextLineSkeleton
            lineClassName="h-4"
            className={cn("h-2.5", i === 1 ? "w-16" : "w-24")}
          />
        </div>
      ))}
    </div>
  );
}

/** Finished tab feed: "Week of …" section headers over the tall cover
 *  cards, in the page's own grid. */
export function FinishedFeedSkeleton() {
  return (
    <div className="flex flex-col gap-10">
      {[4, 6].map((count, s) => (
        <section key={s}>
          <div className="border-border/40 mb-4 flex flex-wrap items-end justify-between gap-2 border-b pb-2">
            <div>
              <TextLineSkeleton lineClassName="h-7" className="h-4 w-32" />
              <TextLineSkeleton className="w-24" />
            </div>
            <TextLineSkeleton className="w-14" />
          </div>
          <div className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-4 sm:grid-cols-[repeat(auto-fill,minmax(220px,1fr))]">
            {Array.from({ length: count }, (_, i) => (
              <div
                key={i}
                className="bg-card ring-foreground/10 flex flex-col overflow-hidden rounded-xl shadow-xs ring-1"
              >
                <Skeleton className="aspect-[2/3] w-full rounded-none" />
                <div className="flex flex-col gap-1.5 p-3.5">
                  <TextLineSkeleton className={lineWidth(i)} />
                  <TextLineSkeleton className="h-3 w-8" />
                  <TextLineSkeleton
                    lineClassName="h-4"
                    className="h-2.5 w-20"
                  />
                </div>
              </div>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

/** `/log/activity` (Finished): intro line + range control + "By week"
 *  select, the four stat tiles, then the weekly cover feed. */
export function FinishedPageSkeleton() {
  return (
    <LoadingRegion label="Loading finished issues…" className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <TextLineSkeleton className="w-64" />
        <div className="flex flex-wrap items-center gap-2 max-sm:w-full">
          <RangeSelectorSkeleton />
          <Skeleton className="h-9 w-32 rounded-md" />
        </div>
      </div>
      <FinishedStatsSkeleton />
      <FinishedFeedSkeleton />
    </LoadingRegion>
  );
}

/** One Stats card shell: `text-lg` title + description over a body. */
function StatCardSkeleton({
  className,
  title = "w-36",
  description = "w-72",
  children,
}: {
  className?: string;
  title?: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <div
      className={cn(
        "border-border bg-card flex min-w-0 flex-col rounded-xl border p-4 sm:p-5",
        className,
      )}
    >
      <div className="mb-4">
        <TextLineSkeleton lineClassName="h-7" className={cn("h-4", title)} />
        <TextLineSkeleton
          lineClassName="mt-0.5 h-5"
          className={cn("max-w-full", description)}
        />
      </div>
      {children}
    </div>
  );
}

/** Stats body under the range row: the six-figure hero band, Weekly
 *  rhythm | Time of day, then the activity chart card. */
export function StatsBodySkeleton() {
  return (
    <div className="space-y-6">
      <div className="border-border bg-border grid grid-cols-2 gap-px overflow-hidden rounded-xl border sm:grid-cols-3 xl:grid-cols-6">
        {Array.from({ length: 6 }, (_, i) => (
          <div key={i} className="bg-card px-4 py-4 sm:px-5 sm:py-5">
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-16" />
            <TextLineSkeleton lineClassName="mt-2 h-9" className="h-7 w-14" />
            <TextLineSkeleton
              lineClassName="mt-1 h-4"
              className={cn("h-2.5", i % 2 ? "w-16" : "w-20")}
            />
          </div>
        ))}
      </div>
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-5">
        <StatCardSkeleton className="lg:col-span-3" description="w-64">
          <Skeleton className="h-[19rem] w-full" />
        </StatCardSkeleton>
        <StatCardSkeleton
          className="lg:col-span-2"
          title="w-28"
          description="w-60"
        >
          <div className="flex flex-1 flex-col items-center gap-6">
            <Skeleton className="size-36 rounded-full" />
            <div className="w-full space-y-2">
              {Array.from({ length: 4 }, (_, i) => (
                <div key={i} className="flex items-center justify-between">
                  <Skeleton className="h-3 w-24" />
                  <Skeleton className="h-3 w-16" />
                </div>
              ))}
            </div>
          </div>
        </StatCardSkeleton>
      </div>
      <StatCardSkeleton title="w-44" description="w-64">
        <Skeleton className="h-[200px] w-full" />
      </StatCardSkeleton>
    </div>
  );
}

/** `/log/stats` route fallback: the sticky range row (caption + range
 *  control, same negative-margin band as `RangeBar`) over the body. */
export function StatsPageSkeleton() {
  return (
    <LoadingRegion label="Loading reading stats…" className="space-y-6">
      <div className="-mx-4 -mt-2 flex items-center justify-between gap-3 border-b border-transparent px-4 py-2 md:-mx-8 md:px-8">
        <TextLineSkeleton className="w-52 max-sm:hidden" />
        <div className="min-w-0 max-sm:flex-1">
          <RangeSelectorSkeleton />
        </div>
      </div>
      <StatsBodySkeleton />
    </LoadingRegion>
  );
}
