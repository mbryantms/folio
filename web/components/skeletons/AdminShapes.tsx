import type * as React from "react";

import {
  LineTabStripSkeleton,
  LoadingRegion,
  TabStripSkeleton,
  TextLineSkeleton,
  lineWidth,
} from "@/components/skeletons/LoadingShapes";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * Admin loading shapes (folio-flair pre-merge polish). Each `…Skeleton`
 * here is the body an admin client component shows while its first
 * query loads, and the `(admin)` route fallback renders the same body
 * under a `PageHeader` stand-in — so the route → client handoff holds
 * still instead of jumping from a generic table to the real layout.
 *
 * The shapes reuse the real `Card` / `CardHeader` / `CardContent` parts
 * with the components' own class strings, and stand text in with
 * `TextLineSkeleton` boxes on the real line heights (text-xs 16px,
 * text-sm 20px, text-base 24px, text-lg 28px, text-2xl 32px), so card
 * paddings, `space-y` + flex-gap stacking and row pitches all match.
 *
 * Server-safe and static: every box sits under the shared shimmer and
 * is hidden from assistive tech; one sr-only label says what is
 * loading.
 */

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/** Text-line stand-in: `line` is the line-box height, `w` the bar width. */
function L({
  line = "h-5",
  w,
  bar = "h-3.5",
  className,
}: {
  line?: string;
  w: string;
  bar?: string;
  className?: string;
}) {
  return (
    <TextLineSkeleton
      lineClassName={cn(line, className)}
      className={cn(bar, "max-w-full", w)}
    />
  );
}

/** A `FilterPill` / chip: text-xs + `py-0.5` + border = 22px. */
function Pill({ w, className }: { w: string; className?: string }) {
  return (
    <Skeleton className={cn("h-[22px] shrink-0 rounded-full", w, className)} />
  );
}

/** A row of pills, optionally led by a small uppercase label. */
function PillRow({
  widths,
  lead,
  className,
}: {
  widths: readonly string[];
  lead?: string;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-wrap items-center gap-2", className)}>
      {lead ? <L line="h-4" bar="h-2.5" w={lead} /> : null}
      {widths.map((w, i) => (
        <Pill key={i} w={w} />
      ))}
    </div>
  );
}

/** A form field: `Label` (14px, leading-none) over a 36px control, with
 *  an optional `text-xs` hint after it. `space-y-2` like `FormItem`. */
function Field({
  label = "w-24",
  control = "h-9",
  hint,
  hintBefore,
  className,
  gap = "space-y-2",
}: {
  label?: string;
  control?: string;
  hint?: string;
  hintBefore?: string;
  className?: string;
  gap?: string;
}) {
  return (
    <div className={cn(gap, className)}>
      <L line="h-3.5" bar="h-3" w={label} />
      {hintBefore ? <L line="h-4" bar="h-3" w={hintBefore} /> : null}
      <Skeleton className={cn("w-full rounded-md", control)} />
      {hint ? <L line="h-4" bar="h-3" w={hint} /> : null}
    </div>
  );
}

/** A settings toggle row: label + description on the left, a switch. */
function SwitchRow({
  label = "w-40",
  lines = ["w-3/4"],
  className,
}: {
  label?: string;
  lines?: readonly string[];
  className?: string;
}) {
  return (
    <div className={cn("flex items-start justify-between gap-6", className)}>
      <div className="min-w-0 flex-1 space-y-1">
        <L line="h-3.5" bar="h-3" w={label} />
        {lines.map((w, i) => (
          <L key={i} line="h-4" bar="h-3" w={w} />
        ))}
      </div>
      <Skeleton className="h-[18px] w-8 shrink-0 rounded-full" />
    </div>
  );
}

/** Header of the dashboard-style cards: uppercase title, `text-xs`
 *  description and a trailing icon (`flex-row … pb-2`). */
function DashCardHeader({
  title = "w-36",
  desc = "w-56",
  icon = true,
}: {
  title?: string;
  desc?: string | null;
  icon?: boolean;
}) {
  return (
    <CardHeader className="flex flex-row items-start justify-between gap-2 pb-2">
      <div className="min-w-0 flex-1">
        <L bar="h-3" w={title} />
        {desc ? <L line="mt-1 h-4" bar="h-2.5" w={desc} /> : null}
      </div>
      {icon ? <Skeleton className="size-4 shrink-0 rounded-sm" /> : null}
    </CardHeader>
  );
}

/** Plain `CardHeader` with an uppercase `text-sm` title (+ optional
 *  `text-xs` description lines). */
function TitleHeader({
  title = "w-32",
  desc,
  className,
}: {
  title?: string;
  desc?: readonly string[];
  className?: string;
}) {
  return (
    <CardHeader className={className}>
      <L bar="h-3" w={title} />
      {desc?.map((w, i) => (
        <L key={i} line="h-4" bar="h-2.5" w={w} />
      ))}
    </CardHeader>
  );
}

/** An outline `size="sm"` button. */
function Btn({ w = "w-full", h = "h-8" }: { w?: string; h?: string }) {
  return <Skeleton className={cn("shrink-0 rounded-md", h, w)} />;
}

/** A bordered table (`DataTable` / `Table` in a `rounded-md border` card):
 *  a 40px header row then body rows of a fixed pitch. `cols` is the grid
 *  template, `widths` the cell bars per column (null = empty cell). */
function TableSkeleton({
  cols,
  head,
  cells,
  rows = 8,
  rowClassName = "h-[37px]",
  className,
}: {
  cols: string;
  head: readonly (string | null)[];
  cells: (row: number) => readonly React.ReactNode[];
  rows?: number;
  rowClassName?: string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "border-border bg-card overflow-hidden rounded-md border",
        className,
      )}
    >
      <div
        className="border-border grid h-10 items-center gap-x-4 border-b px-2"
        style={{ gridTemplateColumns: cols }}
      >
        {head.map((w, i) =>
          w ? <Skeleton key={i} className={cn("h-3", w)} /> : <span key={i} />,
        )}
      </div>
      {Array.from({ length: rows }, (_, r) => (
        <div
          key={r}
          className={cn(
            "border-border grid items-center gap-x-4 border-b px-2 last:border-b-0",
            rowClassName,
          )}
          style={{ gridTemplateColumns: cols }}
        >
          {cells(r).map((c, i) => (
            <div key={i} className="min-w-0">
              {c}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

/** A 2:3 cover placeholder of a fixed width. */
function Cover({ className }: { className: string }) {
  return <Skeleton className={cn("shrink-0 rounded-sm", className)} />;
}

// ---------------------------------------------------------------------------
// Page header
// ---------------------------------------------------------------------------

/** Rough rendered widths, in px, of the admin `PageHeader` text: the
 *  `text-2xl` semibold title (~12.5px a glyph) and the `text-sm`
 *  description (~5.8px a glyph), wrapping past ~1060px. */
const TITLE_PX = 12.5;
const DESC_PX = 5.8;
const DESC_LINE_PX = 1060;

function descLines(chars: number): string[] {
  const px = Math.round(chars * DESC_PX);
  if (px <= DESC_LINE_PX) return [`${px}px`];
  return ["100%", `${Math.max(80, px - DESC_LINE_PX)}px`];
}

/** `PageHeader` stand-in sized to the page's own title and description
 *  (character counts), with optional breadcrumb and action boxes. Same
 *  border, margins and phone tightening as the real header. */
export function AdminHeaderSkeleton({
  title,
  description = 0,
  breadcrumb = false,
  actions,
}: {
  /** Title length in characters. */
  title: number;
  /** Description length in characters (0 = none). */
  description?: number;
  breadcrumb?: boolean;
  actions?: React.ReactNode;
}) {
  return (
    <div
      aria-hidden
      className="border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4 max-sm:mb-4 max-sm:pb-3"
    >
      <div className="min-w-0 flex-1">
        {breadcrumb ? (
          <TextLineSkeleton lineClassName="mb-1.5 h-4" className="h-3 w-28" />
        ) : null}
        <div className="flex h-8 items-center max-sm:h-7">
          <Skeleton
            className="h-6 max-w-full max-sm:h-5"
            style={{ width: `${Math.round(title * TITLE_PX)}px` }}
          />
        </div>
        {description > 0
          ? descLines(description).map((w, i) => (
              <div
                key={i}
                className={cn(
                  "flex h-5 items-center max-sm:h-4",
                  i === 0 && "mt-1",
                )}
              >
                <Skeleton
                  className="h-3.5 max-w-full max-sm:h-3"
                  style={{ width: w }}
                />
              </div>
            ))
          : null}
      </div>
      {actions ? (
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      ) : null}
    </div>
  );
}

/** Line-variant `TabsList` + the `pt-4` panel under it, inside the Tabs
 *  root's `flex-col gap-2`. */
function LineTabsShell({
  widths,
  className,
  children,
}: {
  widths: readonly string[];
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <LineTabStripSkeleton widths={widths} />
      <div className="pt-4">{children}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Dashboard (/admin)
// ---------------------------------------------------------------------------

/** `StatCard` (`components/admin/StatCard`): label, `text-3xl` value,
 *  hint, in a `p-5` card. */
export function StatCardSkeleton({ i = 0 }: { i?: number }) {
  return (
    <Card className="overflow-hidden">
      <CardContent className="flex flex-col gap-2 p-5">
        <L line="h-4" bar="h-2.5" w={i % 2 ? "w-20" : "w-16"} />
        <L line="h-9" bar="h-7" w={i % 2 ? "w-16" : "w-24"} />
        <L line="h-4" bar="h-2.5" w={i % 2 ? "w-28" : "w-20"} />
      </CardContent>
    </Card>
  );
}

/** The four totals tiles (dashboard + stats overview). */
export function StatCardRowSkeleton() {
  return (
    <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
      {[0, 1, 2, 3].map((i) => (
        <StatCardSkeleton key={i} i={i} />
      ))}
    </div>
  );
}

/** "Open health issues" body: the three severity rows. */
export function HealthCountsSkeleton() {
  return (
    <div className="space-y-1">
      {["w-12", "w-16", "w-8"].map((w, i) => (
        <div key={i} className="flex items-center justify-between py-0.5">
          <L line="h-6" w={w} />
          <L line="h-6" bar="h-4" w="w-4" />
        </div>
      ))}
    </div>
  );
}

/** The bordered `Stat` boxes (Scans: 3, Readers: 2). */
export function StatBoxesSkeleton({ count }: { count: 2 | 3 }) {
  return (
    <div
      className={cn("grid gap-3", count === 3 ? "grid-cols-3" : "grid-cols-2")}
    >
      {Array.from({ length: count }, (_, i) => (
        <div
          key={i}
          className="border-border bg-background rounded-md border p-3"
        >
          <L line="h-4" bar="h-2.5" w="w-14" />
          <L line="mt-1 h-7" bar="h-5" w="w-6" />
        </div>
      ))}
    </div>
  );
}

/** Short list rows (`space-y-1`, `py-0.5` text-sm): oldest scans. */
export function CompactRowsSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div className="space-y-1">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center justify-between py-0.5">
          <L w={["w-16", "w-14", "w-10", "w-20", "w-12"][i % 5]!} />
          <L bar="h-3" w="w-10" />
        </div>
      ))}
    </div>
  );
}

/** A centred one-line empty note (`py-4 text-xs`). */
export function EmptyNoteSkeleton({ w = "w-48" }: { w?: string }) {
  return (
    <div className="flex h-12 items-center justify-center">
      <Skeleton className={cn("h-3", w)} />
    </div>
  );
}

/** The 240×56 sparkline. */
export function SparklineSkeleton() {
  return <Skeleton className="h-14 w-full max-w-[240px] rounded-md" />;
}

/** "Service status": three dependency pills, then version + uptime. */
export function ServiceStatusSkeleton() {
  return (
    <div className="space-y-1.5">
      {["w-16", "w-12", "w-20"].map((w, i) => (
        <div key={i} className="flex items-center justify-between">
          <span className="flex items-center gap-2">
            <Skeleton className="size-3.5 rounded-sm" />
            <L w={w} />
          </span>
          <Skeleton className="h-5 w-9 rounded-md" />
        </div>
      ))}
      {["w-12", "w-14"].map((w, i) => (
        <div key={i} className="flex items-center justify-between">
          <L line="h-4" bar="h-2.5" w={w} />
          <L line="h-4" bar="h-2.5" w="w-20" />
        </div>
      ))}
    </div>
  );
}

function DashCard({
  title,
  desc,
  icon = true,
  className = "space-y-3 pt-2",
  children,
}: {
  title: string;
  desc: string;
  icon?: boolean;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <Card>
      <DashCardHeader title={title} desc={desc} icon={icon} />
      <CardContent className={className}>{children}</CardContent>
    </Card>
  );
}

/** `/admin` (`DashboardClient`) while its queries load. */
export function DashboardSkeleton({
  label = "Loading dashboard…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <StatCardRowSkeleton />
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <DashCard title="w-40" desc="w-52">
          <HealthCountsSkeleton />
          <Btn />
        </DashCard>
        <DashCard title="w-14" desc="w-60">
          <StatBoxesSkeleton count={3} />
          <L line="h-4" bar="h-2.5" w="w-32" />
          <div className="flex gap-2">
            <Btn w="flex-1" />
            <Btn w="flex-1" />
          </div>
        </DashCard>
        <DashCard title="w-20" desc="w-52">
          <StatBoxesSkeleton count={2} />
          <Btn />
        </DashCard>
      </div>
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <DashCard title="w-44" desc="w-64" className="space-y-2 pt-2">
          <EmptyNoteSkeleton />
          <Btn />
        </DashCard>
        <DashCard title="w-48" desc="w-60" className="space-y-2 pt-2">
          <CompactRowsSkeleton />
          <Btn />
        </DashCard>
      </div>
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <DashCard title="w-40" desc="w-48">
          <SparklineSkeleton />
          <Btn />
        </DashCard>
        <DashCard title="w-28" desc="w-48">
          <ServiceStatusSkeleton />
        </DashCard>
        <DashCard
          title="w-28"
          desc="w-32"
          icon={false}
          className="space-y-2 pt-2"
        >
          {[0, 1, 2, 3].map((i) => (
            <Btn key={i} />
          ))}
        </DashCard>
      </div>
    </LoadingRegion>
  );
}

// ---------------------------------------------------------------------------
// Activity (/admin/activity, /admin/audit)
// ---------------------------------------------------------------------------

/** Activity feed rows: kind badge + time, then icon, summary, detail. */
export function ActivityRowsSkeleton({
  rows = 8,
  label = "Loading activity…",
}: {
  rows?: number;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <ul className="divide-border border-border bg-card divide-y rounded-md border">
        {Array.from({ length: rows }, (_, i) => (
          <li
            key={i}
            className="grid grid-cols-[8rem_1fr] items-center gap-3 px-3 py-2.5"
          >
            <div className="flex items-center gap-2">
              <Skeleton className="h-5 w-11 rounded-md" />
              <Skeleton className="h-3 w-12" />
            </div>
            <div className="min-w-0">
              <Skeleton className="size-3.5 rounded-sm" />
              <L w={cn(i % 3 === 1 ? "w-40" : "w-56")} />
              <L line="mt-0.5 h-4" bar="h-2.5" w={lineWidth(i)} />
            </div>
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** `/admin/activity`: the "Show" chips over the feed. */
export function ActivityFeedSkeleton() {
  return (
    <div className="space-y-4">
      <PillRow lead="w-9" widths={["w-16", "w-28"]} />
      <ActivityRowsSkeleton />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Auth (/admin/auth)
// ---------------------------------------------------------------------------

function ToggleBox({ lines = 1 }: { lines?: number }) {
  return (
    <div className="border-border flex items-center justify-between rounded-md border p-3">
      <div className="min-w-0 flex-1 space-y-1">
        <L line="h-3.5" bar="h-3" w="w-36" />
        {Array.from({ length: lines }, (_, i) => (
          <L key={i} line="h-4" bar="h-2.5" w={i ? "w-1/3" : "w-4/5"} />
        ))}
      </div>
      <Skeleton className="ml-4 h-[18px] w-8 shrink-0 rounded-full" />
    </div>
  );
}

/** `/admin/auth` (`AuthConfigEditor`): Mode + Local auth cards, the OIDC
 *  card, Save, then the token-TTL card. */
export function AuthConfigSkeleton({
  label = "Loading auth settings…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <div className="space-y-4">
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <Card>
            <TitleHeader title="w-12" />
            <CardContent className="space-y-3">
              <Skeleton className="h-[38px] w-48 rounded-md" />
              <L line="h-4" bar="h-2.5" w="w-11/12" />
              <L line="h-4" bar="h-2.5" w="w-16" />
            </CardContent>
          </Card>
          <Card>
            <TitleHeader title="w-24" />
            <CardContent className="space-y-3">
              <SwitchRow label="w-44" lines={["w-11/12", "w-20"]} />
            </CardContent>
          </Card>
          <Card className="lg:col-span-2">
            <TitleHeader title="w-16" />
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <L line="h-3.5" bar="h-3" w="w-12" />
                <Skeleton className="h-9 w-full rounded-md" />
                <div className="pt-1">
                  <Btn w="w-32" />
                </div>
              </div>
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <Field label="w-16" />
                <Field label="w-20" hint="w-64" />
              </div>
              <ToggleBox />
              <ToggleBox lines={2} />
              <ToggleBox />
              <ToggleBox />
            </CardContent>
          </Card>
        </div>
        <div className="flex items-center justify-end gap-3">
          <Btn w="w-16" h="h-9" />
        </div>
      </div>
      <Card>
        <TitleHeader title="w-20" />
        <CardContent className="space-y-4">
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="w-20" hint="w-56" />
            <Field label="w-24" hint="w-56" />
          </div>
          <div className="flex items-center justify-end gap-3">
            <Btn w="w-16" h="h-9" />
          </div>
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

// ---------------------------------------------------------------------------
// Background work (/admin/background-work)
// ---------------------------------------------------------------------------

/** `/admin/background-work` (`BackgroundWorkClient`): the status line,
 *  the four tiles, library rows (name · scan · covers) and the
 *  other-work card. */
export function BackgroundWorkSkeleton({
  label = "Loading background work…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <div className="space-y-3">
        <div className="flex h-5 items-center gap-2">
          <Skeleton className="size-4 rounded-full" />
          <Skeleton className="h-3.5 w-56" />
        </div>
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {["w-10", "w-28", "w-20", "w-16"].map((w, i) => (
            <Card key={i} className="h-full">
              <CardHeader className="pb-1">
                <L line="h-4" bar="h-2.5" w={w} />
              </CardHeader>
              <CardContent>
                <div>
                  <L line="h-8" bar="h-6" w={i === 1 ? "w-20" : "w-6"} />
                  <L line="mt-0.5 h-4" bar="h-2.5" w="w-24" />
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      </div>
      <Card>
        <CardHeader className="pb-2">
          <L bar="h-3" w="w-16" />
          <L line="h-4" bar="h-2.5" w="w-96" />
        </CardHeader>
        <CardContent>
          <ul className="divide-border divide-y">
            {[0, 1, 2].map((i) => (
              <li
                key={i}
                className="grid gap-x-6 gap-y-3 py-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)_minmax(0,1.4fr)_auto]"
              >
                <div className="min-w-0">
                  <L w={i === 1 ? "w-14" : "w-12"} />
                  <L line="h-4" bar="h-2.5" w="w-16" />
                </div>
                <div className="flex items-center">
                  <Skeleton className="h-3 w-24" />
                </div>
                <div className="min-w-0 space-y-1">
                  <div className="flex h-4 items-center justify-between">
                    <Skeleton className="h-2.5 w-12" />
                    <Skeleton className="h-2.5 w-16" />
                  </div>
                  <Skeleton className="h-1.5 w-full rounded-full" />
                  <L line="h-4" bar="h-2.5" w="w-36" />
                </div>
                <Skeleton className="hidden size-4 self-center rounded-sm md:block" />
              </li>
            ))}
          </ul>
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="pb-2">
          <L bar="h-3" w="w-20" />
          <L line="h-4" bar="h-2.5" w="w-96" />
        </CardHeader>
        <CardContent className="space-y-4">
          <L w="w-28" />
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

// ---------------------------------------------------------------------------
// Duplicates (/admin/duplicates, /admin/libraries/<slug>/duplicates)
// ---------------------------------------------------------------------------

/** Duplicate groups: the "N groups" count, then group cards (kind badge
 *  + series header, copy rows with a cover, path, size and actions). */
export function DuplicateGroupsSkeleton({
  label = "Loading duplicates…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <L line="h-4" bar="h-2.5" w="w-16" />
      <ul className="space-y-3">
        {[2, 3].map((copies, g) => (
          <li key={g} className="border-border bg-card rounded-md border">
            <div className="border-border flex items-center gap-2 border-b px-4 py-2">
              <Skeleton className="h-5 w-24 rounded-md" />
              <L w={g ? "w-28" : "w-20"} />
              <Skeleton className="ml-auto h-3 w-14" />
            </div>
            <ul className="divide-border divide-y">
              {Array.from({ length: copies }, (_, i) => (
                <li key={i} className="flex items-center gap-3 px-4 py-3">
                  <Cover className="h-16 w-11" />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <L w="w-28" />
                    <L line="h-4" bar="h-2.5" w="w-3/4" />
                    <L line="h-4" bar="h-2.5" w="w-24" />
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <Btn w="w-16" />
                    <Btn w="w-14" />
                    <Btn w="w-20" />
                  </div>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** `DuplicatesPanel`: the explainer, kind chips, then the groups. */
export function DuplicatesPanelSkeleton() {
  return (
    <div className="space-y-4">
      <div className="max-w-3xl">
        <L w="w-full" />
        <L w="w-full" />
        <L w="w-2/5" />
      </div>
      <PillRow widths={["w-9", "w-24", "w-24", "w-24"]} />
      <DuplicateGroupsSkeleton />
    </div>
  );
}

/** `/admin/duplicates` (`DuplicatesOverview`): library picker + panel. */
export function DuplicatesOverviewSkeleton() {
  return (
    <div className="space-y-4">
      <div aria-hidden className="flex items-center gap-2">
        <L w="w-12" />
        <Skeleton className="h-8 w-24 rounded-md" />
      </div>
      <DuplicatesPanelSkeleton />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Email (/admin/email)
// ---------------------------------------------------------------------------

/** `/admin/email` (`EmailAdminClient`): the SMTP form card and the
 *  status card beside it. */
export function EmailSkeleton({
  label = "Loading email settings…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion
      label={label}
      className="grid grid-cols-1 gap-4 lg:grid-cols-3"
    >
      <Card className="lg:col-span-2">
        <TitleHeader title="w-40" />
        <CardContent>
          <div className="space-y-4">
            <Field label="w-20" hint="w-96" />
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <Field label="w-8" />
              <Field label="w-8" hint="w-64" />
            </div>
            <Field label="w-16" />
            <Field label="w-16" hint="w-56" />
            <Field label="w-24" hint="w-96" />
            <div className="flex justify-end">
              <Btn w="w-14" h="h-9" />
            </div>
          </div>
        </CardContent>
      </Card>
      <Card>
        <TitleHeader title="w-16" />
        <CardContent className="space-y-4">
          {["w-16", "w-12", "w-6"].map((v, i) => (
            <div key={i}>
              <L line="mb-1 h-4" bar="h-2.5" w={i ? "w-16" : "w-10"} />
              {i === 0 ? (
                <Skeleton className="h-5 w-16 rounded-md" />
              ) : (
                <L w={v} />
              )}
            </div>
          ))}
          <Btn h="h-9" />
          <L line="h-4" bar="h-2.5" w="w-44" />
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

// ---------------------------------------------------------------------------
// Findings (/admin/findings)
// ---------------------------------------------------------------------------

/** Library-event rows (`LibraryEventsList`): severity dot, category
 *  badge + summary, library line, timestamp. */
export function LibraryEventRowsSkeleton({
  rows = 10,
  showLibrary = true,
  label = "Loading events…",
}: {
  rows?: number;
  showLibrary?: boolean;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <ul className="divide-border border-border bg-card divide-y rounded-md border">
        {Array.from({ length: rows }, (_, i) => (
          <li key={i} className="px-3 py-2">
            <div className="flex items-start gap-3">
              <Skeleton className="mt-1.5 size-2 shrink-0 rounded-full" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <Skeleton className="h-5 w-24 shrink-0 rounded-md" />
                  <L w={lineWidth(i)} />
                </div>
                {showLibrary ? (
                  <L line="mt-0.5 h-4" bar="h-2.5" w="w-10" />
                ) : null}
              </div>
              <Skeleton className="mt-0.5 h-3 w-32 shrink-0" />
            </div>
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** The event filter chips (severities | categories). */
function EventChipsSkeleton() {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {["w-10", "w-16", "w-12"].map((w, i) => (
        <Pill key={i} w={w} />
      ))}
      <span className="bg-border mx-1 h-4 w-px" />
      {[
        "w-12",
        "w-14",
        "w-9",
        "w-12",
        "w-[4.5rem]",
        "w-16",
        "w-14",
        "w-12",
        "w-10",
      ].map((w, i) => (
        <Pill key={i} w={w} />
      ))}
    </div>
  );
}

/** Cross-library health issue cards (findings "Open health issues"). */
export function HealthIssueCardsSkeleton({
  rows = 5,
  label = "Loading health issues…",
}: {
  rows?: number;
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-2">
      {Array.from({ length: rows }, (_, i) => (
        <Card key={i}>
          <CardContent className="flex flex-row items-start gap-3 py-3">
            <Skeleton className="mt-0.5 size-4 shrink-0 rounded-full" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <L w="w-36" />
                <Skeleton className="h-3 w-28" />
                <Skeleton className="h-5 w-14 rounded-md" />
                <Skeleton className="ml-auto h-3 w-32" />
              </div>
              <L line="mt-1 h-5" bar="h-2.5" w={lineWidth(i)} />
            </div>
            <Btn w="w-16" />
          </CardContent>
        </Card>
      ))}
    </LoadingRegion>
  );
}

/** Cross-library scan-run table (findings "Scan runs"). */
export function ScanRunsCardSkeleton({
  label = "Loading scan runs…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <Card>
        <CardContent className="p-0">
          <TableSkeleton
            className="rounded-none border-0"
            cols="6rem 7rem 9rem 10rem 10rem minmax(0,1fr) 3rem"
            head={["w-10", "w-12", "w-8", "w-14", "w-12", "w-10", null]}
            cells={() => [
              <Skeleton key="s" className="h-5 w-20 rounded-md" />,
              <Skeleton key="l" className="h-3.5 w-16" />,
              <Skeleton key="k" className="h-3 w-20" />,
              <Skeleton key="t" className="h-3 w-28" />,
              <Skeleton key="e" className="h-3 w-28" />,
              <span key="x" />,
              <Skeleton key="g" className="h-3 w-10" />,
            ]}
          />
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

/** `/admin/findings` (`FindingsView`, default Activity tab). */
export function FindingsSkeleton() {
  return (
    <LineTabsShell widths={["w-14", "w-32", "w-20"]}>
      <div className="space-y-3">
        <div className="border-border/60 flex flex-wrap items-center gap-2 rounded-md border p-3">
          <L line="h-4" bar="h-2.5" w="w-12" />
          <Skeleton className="h-8 w-28 rounded-md" />
        </div>
        <div className="space-y-3">
          <EventChipsSkeleton />
          <LibraryEventRowsSkeleton />
        </div>
      </div>
    </LineTabsShell>
  );
}

// ---------------------------------------------------------------------------
// Libraries (/admin/libraries and /admin/libraries/<slug>/*)
// ---------------------------------------------------------------------------

/** `/admin/libraries` (`LibraryList`): library cards. */
export function LibraryListSkeleton({
  count = 3,
  label = "Loading libraries…",
}: {
  count?: number;
  label?: string;
}) {
  return (
    <LoadingRegion
      label={label}
      className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3"
    >
      {Array.from({ length: count }, (_, i) => (
        <Card key={i} className="h-full">
          <CardContent className="flex h-full flex-col gap-2 p-5">
            <L line="h-6" w={i % 2 ? "w-16" : "w-12"} />
            <L line="h-4" bar="h-2.5" w="w-48" />
            <L line="h-4" bar="h-2.5" w="w-56" />
            <div className="mt-auto flex items-center justify-between pt-2">
              <L line="h-4" bar="h-2.5" w="w-40" />
              <Skeleton className="h-[22px] w-10 rounded-full" />
            </div>
          </CardContent>
        </Card>
      ))}
    </LoadingRegion>
  );
}

/** The library detail layout's header (breadcrumb, name, root path) and
 *  its underline section tabs. Route fallback only — the layout renders
 *  them server-side once the library resolves. */
export function LibraryChromeSkeleton({ active = 0 }: { active?: number }) {
  return (
    <>
      <div aria-hidden className="space-y-1">
        <L line="mb-1 h-4" bar="h-3" w="w-28" />
        <L line="h-8" bar="h-6" w="w-24" />
        <L line="h-4" bar="h-2.5" w="w-44" />
      </div>
      <div
        aria-hidden
        className="border-border relative -mb-px flex gap-1 overflow-hidden border-b"
      >
        {["w-14", "w-14", "w-11", "w-11", "w-[4.5rem]", "w-14", "w-14"].map(
          (w, i) => (
            <div
              key={i}
              className={cn(
                "flex shrink-0 items-center border-b-2 px-3 py-2",
                i === active ? "border-muted" : "border-transparent",
              )}
            >
              <L w={w} />
            </div>
          ),
        )}
      </div>
    </>
  );
}

/** "Contents and disk usage" figures (`LibraryStatsCard` body). */
export function LibraryStatsBodySkeleton() {
  return (
    <div aria-hidden>
      <div className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-3 lg:grid-cols-6">
        {["w-10", "w-10", "w-24", "w-20", "w-24", "w-20"].map((w, i) => (
          <div key={i} className="min-w-0">
            <L line="h-4" bar="h-2.5" w={w} />
            <L w={i > 3 ? "w-32" : "w-12"} />
          </div>
        ))}
      </div>
      <L line="mt-3 h-4" bar="h-2.5" w="w-3/4" />
    </div>
  );
}

/** "Archive backups (.bak)" figures (`BackupStorageCard` body). */
export function BackupStatsBodySkeleton() {
  return (
    <div aria-hidden>
      <div className="grid grid-cols-2 gap-x-6 gap-y-2 sm:grid-cols-4">
        {["w-14", "w-8", "w-12", "w-12"].map((w, i) => (
          <div key={i} className="min-w-0">
            <L line="h-4" bar="h-2.5" w={w} />
            <L w={i === 1 ? "w-8" : "w-20"} />
          </div>
        ))}
      </div>
      <L line="mt-2 h-4" bar="h-2.5" w="w-80" />
    </div>
  );
}

function IconTitleHeader({ title }: { title: string }) {
  return (
    <CardHeader className="pb-2">
      <div className="flex h-5 items-center gap-2">
        <Skeleton className="size-4 rounded-sm" />
        <Skeleton className={cn("h-3.5", title)} />
      </div>
    </CardHeader>
  );
}

/** Library Overview tab (`LibraryOverview`): the four stat cards, the
 *  contents card, the scanner-status grid and the scan buttons. */
export function LibraryOverviewSkeleton({
  label = "Loading library…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {["w-16", "w-16", "w-28", "w-32"].map((w, i) => (
          <Card key={i}>
            <CardContent className="space-y-1 p-4">
              <L line="h-4" bar="h-2.5" w={w} />
              <L line="h-7" bar="h-4" w={i === 0 ? "w-36" : "w-16"} />
            </CardContent>
          </Card>
        ))}
      </div>
      <Card>
        <IconTitleHeader title="w-40" />
        <CardContent>
          <LibraryStatsBodySkeleton />
        </CardContent>
      </Card>
      <Card>
        <CardContent className="grid gap-4 p-4 md:grid-cols-[1.4fr_1fr_1fr_1fr]">
          <div className="space-y-1">
            <L line="h-4" bar="h-2.5" w="w-28" />
            <L w="w-11/12" />
            <L w="w-1/3" />
            <L line="h-4" bar="h-2.5" w="w-28" />
          </div>
          {["w-16", "w-24", "w-24", "w-24", "w-36", "w-32", "w-36"].map(
            (w, i) => (
              <div key={i} className="min-w-0 space-y-1">
                <L line="h-4" bar="h-2.5" w={w} />
                <L w={i === 4 ? "w-24" : "w-14"} />
              </div>
            ),
          )}
        </CardContent>
      </Card>
      <div className="flex flex-wrap items-center gap-3">
        <Btn w="w-20" h="h-9" />
        <Skeleton className="h-[30px] w-28 rounded-md" />
      </div>
    </LoadingRegion>
  );
}

function FormCard({ children }: { children: React.ReactNode }) {
  return (
    <Card>
      <CardContent className="space-y-5 p-6">{children}</CardContent>
    </Card>
  );
}

function SectionHeading({ desc = 2 }: { desc?: number }) {
  return (
    <div>
      <L bar="h-3.5" w="w-36" />
      {Array.from({ length: desc }, (_, i) => (
        <L
          key={i}
          line={cn("h-4", i === 0 && "mt-1")}
          bar="h-2.5"
          w={i === desc - 1 ? "w-1/2" : "w-full"}
        />
      ))}
    </div>
  );
}

/** Library Settings tab (`LibrarySettingsForm`): the form's cards
 *  (scan, scanner, writeback, metadata), Save row and the thumbnail
 *  card. */
export function LibrarySettingsSkeleton({
  label = "Loading library settings…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <FormCard>
        <Field label="w-20" hintBefore="w-80" />
        <div className="space-y-2">
          <L line="h-3.5" bar="h-3" w="w-24" />
          <L line="h-4" bar="h-3" w="w-72" />
          <Skeleton className="h-9 w-full rounded-md" />
          <Skeleton className="h-9 w-full rounded-md" />
        </div>
        <SwitchRow label="w-40" lines={["w-full", "w-24"]} />
      </FormCard>
      <FormCard>
        <SwitchRow label="w-40" lines={["w-72"]} />
        <SwitchRow label="w-36" lines={["w-full", "w-40"]} />
        <SwitchRow label="w-44" lines={["w-3/4"]} />
        <SwitchRow label="w-40" lines={["w-2/3"]} />
        <SwitchRow label="w-48" lines={["w-3/4"]} />
        <Field label="w-32" hint="w-96" className="max-w-xs" />
      </FormCard>
      <FormCard>
        <SectionHeading />
        <SwitchRow label="w-44" lines={["w-80"]} />
        <SwitchRow label="w-48" lines={["w-3/4"]} />
        <SwitchRow label="w-40" lines={["w-2/3"]} />
        <SwitchRow label="w-44" lines={["w-3/4"]} />
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <Field label="w-40" hint="w-full" />
          <Field label="w-40" hint="w-full" />
        </div>
      </FormCard>
      <FormCard>
        <Field label="w-48" hintBefore="w-full" control="h-9" />
        <SwitchRow label="w-40" lines={["w-3/4"]} />
        <SwitchRow label="w-44" lines={["w-2/3"]} />
      </FormCard>
      <div className="flex justify-end gap-2">
        <Btn w="w-16" h="h-9" />
        <Btn w="w-28" h="h-9" />
      </div>
      <ThumbnailCardShape />
    </LoadingRegion>
  );
}

/** The thumbnail-settings card while its settings load. */
export function ThumbnailSettingsSkeleton() {
  return (
    <LoadingRegion label="Loading thumbnail settings…">
      <ThumbnailCardShape />
    </LoadingRegion>
  );
}

function ThumbnailCardShape() {
  return (
    <FormCard>
      <SectionHeading desc={1} />
      <SwitchRow label="w-36" lines={["w-3/4"]} />
      <div className="space-y-2">
        <L bar="h-3" w="w-24" />
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-[58px] rounded-md" />
          ))}
        </div>
      </div>
      <div className="grid grid-cols-1 gap-5 md:grid-cols-2">
        <Field label="w-28" control="h-4" hint="w-64" />
        <Field label="w-36" control="h-4" hint="w-64" />
      </div>
    </FormCard>
  );
}

/** Library Health tab (`HealthIssuesTable`): backups card, deep-validate
 *  row, status/severity and kind chips, then the issues table. */
export function LibraryHealthSkeleton({
  label = "Loading health issues…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <Card>
        <IconTitleHeader title="w-36" />
        <CardContent>
          <BackupStatsBodySkeleton />
        </CardContent>
      </Card>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="max-w-prose min-w-0 flex-1">
          {["w-full", "w-full", "w-full", "w-1/3"].map((w, i) => (
            <L key={i} line="h-4" bar="h-2.5" w={w} />
          ))}
        </div>
        <Btn w="w-40" />
      </div>
      <PillRow
        widths={["w-14", "w-20", "w-20", "w-12", "w-9", "w-12", "w-16", "w-10"]}
      />
      <PillRow lead="w-8" widths={["w-16", "w-24", "w-40", "w-36", "w-40"]} />
      <TableSkeleton
        cols="5rem 8rem minmax(0,1fr) 9rem 9rem 5rem"
        head={["w-14", "w-8", "w-16", "w-16", "w-16", null]}
        rows={6}
        rowClassName="h-[72px]"
        cells={(r) => [
          <Skeleton key="s" className="h-5 w-14 rounded-md" />,
          <div key="k" className="space-y-1">
            <Skeleton className="h-3 w-20" />
            <Skeleton className="h-2.5 w-24" />
          </div>,
          <div key="m" className="space-y-1.5">
            <Skeleton className={cn("h-3", lineWidth(r))} />
            <Skeleton className="h-3 w-1/3" />
          </div>,
          <Skeleton key="f" className="h-3 w-28" />,
          <Skeleton key="l" className="h-3 w-28" />,
          <Skeleton key="d" className="h-3 w-12" />,
        ]}
      />
    </LoadingRegion>
  );
}

/** Library History tab (`ScanRunsTable`): the segmented kind filter and
 *  the expandable scan-run table. */
export function ScanRunsSkeleton({
  label = "Loading scan history…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="flex flex-col gap-4">
      <TabStripSkeleton tabs={4} />
      <TableSkeleton
        cols="2.25rem 10rem 6rem 6rem 5rem minmax(0,1fr)"
        head={[null, "w-14", "w-10", "w-10", "w-14", "w-14"]}
        rows={10}
        rowClassName="h-[51px]"
        cells={(r) => [
          <Skeleton key="x" className="size-3.5 rounded-sm" />,
          <Skeleton key="t" className="h-3 w-32" />,
          <div key="k" className="space-y-1">
            <Skeleton className="h-4 w-14 rounded-md" />
            {r % 3 ? <Skeleton className="h-2.5 w-20" /> : null}
          </div>,
          <Skeleton key="s" className="h-5 w-20 rounded-md" />,
          <Skeleton key="d" className="h-3 w-10" />,
          <Skeleton key="r" className={cn("h-3", r % 3 ? "w-20" : "w-56")} />,
        ]}
      />
    </LoadingRegion>
  );
}

/** Library Removed tab (`RemovedItemsTable`): the bulk bar, the removed
 *  series table and the removed issues table. */
export function RemovedItemsSkeleton({
  label = "Loading removed items…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <div className="border-border bg-card flex flex-wrap items-center justify-between gap-3 rounded-md border px-4 py-3">
        <L w="w-[36rem]" />
        <div className="flex items-center gap-2">
          <Btn w="w-32" />
          <Btn w="w-36" />
        </div>
      </div>
      <section className="space-y-2">
        <L line="h-4" bar="h-2.5" w="w-16" />
        <TableSkeleton
          cols="2.5rem minmax(0,1fr) minmax(0,1.6fr) 3rem 9rem 5rem 8rem"
          head={[null, "w-10", "w-12", "w-10", "w-16", "w-10", "w-14"]}
          rows={1}
          rowClassName="h-[41px]"
          cells={() => [
            <Skeleton key="c" className="size-4 rounded-sm" />,
            <Skeleton key="n" className="h-3.5 w-40" />,
            <Skeleton key="f" className="h-3 w-full" />,
            <Skeleton key="i" className="h-3 w-4" />,
            <Skeleton key="r" className="h-3 w-28" />,
            <Skeleton key="s" className="h-5 w-14 rounded-md" />,
            <Skeleton key="a" className="ml-auto h-8 w-28 rounded-md" />,
          ]}
        />
      </section>
      <section className="space-y-2">
        <L line="h-4" bar="h-2.5" w="w-16" />
        <L line="h-4" bar="h-2.5" w="w-[28rem]" />
        <TableSkeleton
          cols="2.5rem minmax(0,1fr) 9rem 5rem 12rem"
          head={[null, "w-8", "w-16", "w-10", "w-14"]}
          rows={2}
          rowClassName="h-[41px]"
          cells={() => [
            <Skeleton key="c" className="size-4 rounded-sm" />,
            <Skeleton key="f" className="h-3 w-3/4" />,
            <Skeleton key="r" className="h-3 w-28" />,
            <Skeleton key="s" className="h-5 w-14 rounded-md" />,
            <Skeleton key="a" className="ml-auto h-8 w-40 rounded-md" />,
          ]}
        />
      </section>
    </LoadingRegion>
  );
}

/** Library Live-scan tab (`LiveScanProgress`): status bar, phase card and
 *  the eight metric tiles. Route fallback only (the client renders its
 *  idle state straight away). */
export function LiveScanSkeleton() {
  return (
    <LoadingRegion label="Loading live scan…" className="space-y-6">
      <Card>
        <CardContent className="flex flex-row flex-wrap items-center gap-4 p-5">
          <Skeleton className="h-6 w-20 rounded-full" />
          <Skeleton className="ml-auto h-3 w-36" />
          <Btn w="w-20" h="h-9" />
        </CardContent>
      </Card>
      <Card>
        <CardContent className="space-y-4 p-5">
          <div className="flex items-baseline justify-between gap-3">
            <div>
              <L bar="h-3.5" w="w-28" />
              <L line="mt-1 h-4" bar="h-2.5" w="w-12" />
            </div>
            <Skeleton className="h-3 w-14" />
          </div>
          <Skeleton className="h-2 w-full rounded-full" />
          <div className="flex flex-wrap gap-2">
            {["w-16", "w-16", "w-20", "w-20", "w-16"].map((w, i) => (
              <Skeleton key={i} className={cn("h-5 rounded-md", w)} />
            ))}
          </div>
        </CardContent>
      </Card>
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        {Array.from({ length: 8 }, (_, i) => (
          <Card key={i}>
            <CardContent className="flex flex-row items-center gap-3 p-4">
              <Skeleton className="size-4 shrink-0 rounded-sm" />
              <div className="min-w-0">
                <L line="h-[15px]" bar="h-2" w="w-20" />
                <L line="h-6" bar="h-4" w="w-6" />
              </div>
            </CardContent>
          </Card>
        ))}
      </div>
    </LoadingRegion>
  );
}

/** `/admin/libraries/<slug>[/tab]` route fallback: the layout's header
 *  and tabs, then the tab's own body inside the `pt-4` panel. */
export function LibraryDetailSkeleton({ tab }: { tab: string }) {
  const TABS = [
    "",
    "settings",
    "health",
    "history",
    "duplicates",
    "removed",
    "scan",
  ];
  const at = Math.max(0, TABS.indexOf(tab === "thumbnails" ? "scan" : tab));
  let body: React.ReactNode;
  switch (TABS[at]) {
    case "settings":
      body = <LibrarySettingsSkeleton />;
      break;
    case "health":
      body = <LibraryHealthSkeleton />;
      break;
    case "history":
      body = <ScanRunsSkeleton />;
      break;
    case "duplicates":
      body = <DuplicatesPanelSkeleton />;
      break;
    case "removed":
      body = <RemovedItemsSkeleton />;
      break;
    case "scan":
      body = <LiveScanSkeleton />;
      break;
    default:
      body = <LibraryOverviewSkeleton />;
  }
  return (
    <div className="space-y-6">
      <LibraryChromeSkeleton active={at} />
      <div className="pt-4">{body}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Logs (/admin/logs)
// ---------------------------------------------------------------------------

/** Log rows: time, level chip, message + target + field chips. */
export function LogRowsSkeleton({
  rows = 12,
  label = "Loading logs…",
}: {
  rows?: number;
  label?: string;
}) {
  return (
    <LoadingRegion label={label} rootClassName="max-h-[70vh] overflow-hidden">
      <ol className="divide-border divide-y">
        {Array.from({ length: rows }, (_, i) => (
          <li
            key={i}
            className="grid grid-cols-[5rem_8rem_1fr] items-start gap-3 px-3 py-2"
          >
            <L line="h-4" bar="h-2.5" w="w-14" />
            <Skeleton className="h-5 w-11 rounded-md" />
            <div className="min-w-0">
              <L line="h-4" bar="h-3" w={i % 2 ? "w-28" : "w-36"} />
              <L line="mt-0.5 h-[15px]" bar="h-2" w="w-32" />
              <div className="mt-1 flex flex-wrap gap-1">
                {["w-52", "w-20", "w-24", "w-24", "w-20"].map((w, k) => (
                  <Skeleton key={k} className={cn("h-[19px] rounded", w)} />
                ))}
              </div>
            </div>
          </li>
        ))}
      </ol>
    </LoadingRegion>
  );
}

/** `/admin/logs` (`LogsClient`): stream / level segments, search, library
 *  picker, follow toggle + count, the log card and the footnote. */
export function LogsSkeleton() {
  return (
    <div className="space-y-4">
      <div
        aria-hidden
        className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between"
      >
        <div className="flex flex-wrap items-center gap-3">
          <Skeleton className="h-[38px] w-[11.5rem] rounded-md" />
          <Skeleton className="h-[38px] w-[17rem] rounded-md" />
          <Skeleton className="h-9 w-72 rounded-md" />
          <div className="flex items-center gap-1.5">
            <Skeleton className="h-2.5 w-12" />
            <Skeleton className="h-8 w-28 rounded-md" />
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Btn w="w-[6.5rem]" />
          <LogCountBadgeSkeleton />
        </div>
      </div>
      <div className="border-border bg-card rounded-md border">
        <LogRowsSkeleton />
      </div>
      <L line="h-4" bar="h-2.5" w="w-[38rem]" />
    </div>
  );
}

/** The "N of capacity" badge beside the follow toggle, held while the
 *  first page loads so the toolbar wraps the same way it will. */
export function LogCountBadgeSkeleton() {
  return <Skeleton aria-hidden className="h-5 w-[5.5rem] rounded-md" />;
}

// ---------------------------------------------------------------------------
// Metadata (/admin/metadata)
// ---------------------------------------------------------------------------

/** Rows of a divided card list: a two-line left block + a right note. */
function CardListRows({
  rows,
  pad = "py-2",
  badge = true,
}: {
  rows: number;
  pad?: string;
  badge?: boolean;
}) {
  return (
    <>
      {Array.from({ length: rows }, (_, i) => (
        <div
          key={i}
          className={cn(
            "border-border flex items-center justify-between gap-3 border-b last:border-0",
            pad,
          )}
        >
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <L w={["w-20", "w-14", "w-40"][i % 3]!} />
              {badge ? <Skeleton className="h-5 w-14 rounded-md" /> : null}
            </div>
          </div>
          <Skeleton className="h-3 w-32 shrink-0" />
        </div>
      ))}
    </>
  );
}

/** Metadata Dashboard tab (`DashboardTab`): three stat cards, provider
 *  quota, recent applies. */
export function MetadataDashboardSkeleton({
  label = "Loading metadata…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <section className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {["w-28", "w-20", "w-20"].map((w, i) => (
          <Card key={i}>
            <CardContent className="space-y-1 p-4">
              <div className="flex h-4 items-center gap-1.5">
                {i < 2 ? <Skeleton className="size-4 rounded-full" /> : null}
                <Skeleton className={cn("h-2.5", w)} />
              </div>
              <L line="h-8" bar="h-6" w={i ? "w-6" : "w-36"} />
              <L line="h-4" bar="h-2.5" w="w-10" />
            </CardContent>
          </Card>
        ))}
      </section>
      <Card>
        <CardHeader className="pb-2">
          <L bar="h-3" w="w-24" />
        </CardHeader>
        <CardContent className="space-y-2">
          <CardListRows rows={3} />
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between gap-2 pb-2">
          <L bar="h-3" w="w-24" />
          <Skeleton className="h-3 w-20" />
        </CardHeader>
        <CardContent>
          <RecentAppliesRowsSkeleton />
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

/** "Recent applies" rows (name + badge, providers line, when). */
export function RecentAppliesRowsSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div aria-hidden className="divide-border/60 divide-y">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center justify-between gap-3 py-2">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <L w={i % 2 ? "w-24" : "w-20"} />
              <Skeleton className="h-5 w-14 rounded-md" />
            </div>
            <L line="h-4" bar="h-2.5" w="w-28" />
          </div>
          <Skeleton className="h-3 w-14 shrink-0" />
        </div>
      ))}
    </div>
  );
}

/** `/admin/metadata` (`AdminMetadataTabs`, Dashboard tab). */
export function MetadataTabsSkeleton() {
  return (
    <LineTabsShell
      widths={["w-[4.5rem]", "w-16", "w-12", "w-16", "w-20", "w-9", "w-14"]}
    >
      <MetadataDashboardSkeleton />
    </LineTabsShell>
  );
}

// ---------------------------------------------------------------------------
// Queue (/admin/queue)
// ---------------------------------------------------------------------------

/** Queue Overview (`QueueLanes`): the counts line + queue filter pills,
 *  then the Queued / Running / Done lanes of job cards. */
export function QueueLanesSkeleton({
  label = "Loading jobs…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <L w="w-72" />
        <div className="flex flex-wrap gap-1.5">
          {["w-12", "w-16", "w-[4.5rem]", "w-20", "w-16"].map((w, i) => (
            <Skeleton key={i} className={cn("h-8 rounded-full", w)} />
          ))}
        </div>
      </div>
      <div className="grid grid-cols-1 items-start gap-3 lg:grid-cols-3">
        {[2, 1, 4].map((cards, lane) => (
          <div key={lane} className="bg-muted/30 min-w-0 rounded-xl border p-3">
            <div className="mb-2.5 flex h-5 items-center justify-between px-0.5">
              <Skeleton className="h-3.5 w-16" />
              <Skeleton className="h-3 w-4" />
            </div>
            <ul className="space-y-2">
              {Array.from({ length: cards }, (_, i) => (
                <li
                  key={i}
                  className="bg-card flex gap-3 rounded-lg border p-2.5"
                >
                  <Skeleton className="h-[54px] w-9 shrink-0 rounded" />
                  <div className="min-w-0 flex-1">
                    <L line="h-4" bar="h-2.5" w="w-24" />
                    <L line="mt-0.5 h-5" w="w-28" />
                    <L line="mt-0.5 h-4" bar="h-2.5" w="w-20" />
                  </div>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </LoadingRegion>
  );
}

/** `/admin/queue` (`QueuePage`, Overview tab). */
export function QueuePageSkeleton() {
  return (
    <div className="mt-2 flex flex-col gap-2">
      <LineTabStripSkeleton widths={["w-16", "w-20", "w-32"]} />
      <div className="mt-4">
        <QueueLanesSkeleton />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Relationships (/admin/relationships)
// ---------------------------------------------------------------------------

/** One end of a suggestion: cover thumb, series name, year. */
function SuggestionEnd({ w }: { w: string }) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Cover className="h-[60px] w-10" />
      <div className="min-w-0">
        <L w={w} />
        <L line="h-4" bar="h-2.5" w="w-8" />
      </div>
    </div>
  );
}

/** Suggestion cards (`SuggestionRow`): the two series ends either side
 *  of the kind badge, confidence, then reason + evidence/actions. */
export function SuggestionCardsSkeleton({
  rows = 4,
  label = "Loading suggestions…",
}: {
  rows?: number;
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <L line="h-4" bar="h-2.5" w="w-24" />
      <ul className="space-y-3">
        {Array.from({ length: rows }, (_, i) => (
          <li key={i} className="border-border bg-card rounded-md border">
            <div className="flex items-start gap-3 px-4 py-3">
              <div className="grid min-w-0 flex-1 gap-2 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] sm:items-center">
                <SuggestionEnd w={i % 2 ? "w-36" : "w-44"} />
                <Skeleton className="h-5 w-16 rounded-md sm:justify-self-center" />
                <SuggestionEnd w={i % 2 ? "w-28" : "w-24"} />
              </div>
              <div className="flex shrink-0 flex-col items-end gap-1">
                <Skeleton className="h-3 w-8" />
                <Skeleton className="h-5 w-10 rounded-md" />
              </div>
            </div>
            <div className="border-border space-y-2 border-t px-4 py-3">
              <L w={lineWidth(i)} />
              <div className="flex items-center gap-2">
                <Skeleton className="h-3 w-28" />
                <div className="ml-auto flex items-center gap-1">
                  <Btn w="w-20" />
                  <Btn w="w-20" />
                  <Btn w="w-16" />
                </div>
              </div>
            </div>
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** `/admin/relationships` (`RelationshipSuggestionsPanel`): library
 *  picker + run button, status and confidence chips, then the cards. */
export function RelationshipsSkeleton() {
  return (
    <div className="space-y-4">
      <div aria-hidden className="flex flex-wrap items-center gap-3">
        <div className="flex items-center gap-2">
          <L w="w-12" />
          <Skeleton className="h-8 w-28 rounded-md" />
        </div>
        <Btn w="w-40" />
      </div>
      <div aria-hidden className="space-y-2">
        <PillRow
          widths={["w-16", "w-[4.5rem]", "w-16", "w-16", "w-12", "w-8"]}
        />
        <PillRow widths={["w-28", "w-16", "w-20", "w-16"]} />
      </div>
      <SuggestionCardsSkeleton />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Scan dashboard (/admin/scan-dashboard)
// ---------------------------------------------------------------------------

/** File-watcher rows (`WatchersCard` body): mode badge, library name,
 *  last trigger. */
export function WatcherRowsSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <LoadingRegion label="Loading watchers…">
      <ul className="divide-border divide-y">
        {Array.from({ length: rows }, (_, i) => (
          <li
            key={i}
            className="flex flex-col gap-1 py-2 sm:flex-row sm:items-center sm:justify-between sm:gap-3"
          >
            <span className="flex min-w-0 items-center gap-2">
              <Skeleton className="h-5 w-14 shrink-0 rounded-md" />
              <L w={["w-12", "w-14", "w-8"][i % 3]!} />
            </span>
            <Skeleton className="h-3 w-3" />
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** Scan-all batch rows (state pill · library count · date). */
export function BatchListSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <LoadingRegion label="Loading scan batches…">
      <ul className="space-y-2">
        {Array.from({ length: rows }, (_, i) => (
          <li
            key={i}
            className="border-border bg-card flex items-center justify-between rounded-md border px-4 py-3"
          >
            <span className="flex items-center gap-3">
              <Skeleton className="h-5 w-16 rounded-full" />
              <Skeleton className="h-4 w-24" />
            </span>
            <Skeleton className="h-3 w-28" />
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** `/admin/scan-dashboard` (`ScanDashboardClient`, no batch selected):
 *  start-scan bar + idle note, file watchers, scan-all history. */
export function ScanDashboardSkeleton() {
  return (
    <div className="space-y-8">
      <div aria-hidden className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <Skeleton className="h-9 w-52 rounded-md" />
          <Btn w="w-28" h="h-9" />
          <Btn w="w-24" h="h-9" />
          <Skeleton className="ml-auto h-3 w-20" />
        </div>
        <div className="border-border/60 flex h-[102px] items-center justify-center rounded-xl border border-dashed px-6">
          <Skeleton className="h-3.5 w-[34rem] max-w-full" />
        </div>
      </div>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between pb-2">
          <div className="flex h-5 items-center gap-2">
            <Skeleton className="size-4 rounded-sm" />
            <Skeleton className="h-3.5 w-24" />
          </div>
          <Skeleton className="h-3 w-40" />
        </CardHeader>
        <CardContent>
          <WatcherRowsSkeleton />
        </CardContent>
      </Card>
      <section className="space-y-3">
        <L line="h-7" bar="h-4" w="w-36" />
        <BatchListSkeleton rows={2} />
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Series (/admin/series/<slug>)
// ---------------------------------------------------------------------------

/** `/admin/series/<slug>` (`SeriesIdentityForm`): the eyebrow header,
 *  identity-override card, scan card and the merge/split note. */
export function SeriesIdentitySkeleton({
  label = "Loading series…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <div>
        <L line="h-4" bar="h-2.5" w="w-12" />
        <L line="h-8" bar="h-6" w="w-48" />
        <L line="h-4" bar="h-2.5" w="w-80" />
      </div>
      <Card>
        <CardContent className="space-y-4 p-5">
          <div className="space-y-2">
            <L line="h-3.5" bar="h-3" w="w-48" />
            <L line="h-4" bar="h-2.5" w="w-80" />
            <div className="flex gap-2">
              <Skeleton className="h-9 flex-1 rounded-md" />
              <Btn w="w-14" h="h-9" />
            </div>
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardContent className="flex flex-row items-center justify-between gap-4 p-5">
          <div className="space-y-1">
            <L line="h-6" w="w-24" />
            <L line="h-4" bar="h-2.5" w="w-80" />
          </div>
          <Btn w="w-24" h="h-9" />
        </CardContent>
      </Card>
      <Card>
        <CardContent className="space-y-2 p-5">
          <L line="h-6" w="w-48" />
          <div>
            {["w-full", "w-full", "w-1/2"].map((w, i) => (
              <L key={i} line="h-[19.5px]" bar="h-2.5" w={w} />
            ))}
          </div>
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

// ---------------------------------------------------------------------------
// Server (/admin/server)
// ---------------------------------------------------------------------------

function KvRows({
  rows,
  badge = false,
}: {
  rows: readonly string[];
  badge?: boolean;
}) {
  return (
    <div className="space-y-2">
      {rows.map((w, i) => (
        <div key={i} className="flex items-center justify-between gap-3">
          <span className="flex items-center gap-2">
            {badge ? <Skeleton className="size-3.5 rounded-sm" /> : null}
            <L w={w} />
          </span>
          {badge ? (
            <Skeleton className="h-5 w-9 rounded-md" />
          ) : (
            <L bar="h-3" w="w-24" />
          )}
        </div>
      ))}
    </div>
  );
}

function InfoCard({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <Card>
      <TitleHeader title={title} />
      <CardContent>{children}</CardContent>
    </Card>
  );
}

/** The OCR models list (`OcrModelsCard` body). */
export function OcrModelsBodySkeleton() {
  return (
    <div aria-hidden className="space-y-2">
      {["w-32", "w-20", "w-24"].map((w, i) => (
        <div key={i} className="flex items-center justify-between">
          <L w={w} />
          <span className="flex items-center gap-2">
            <Skeleton className="h-3 w-10" />
            <Skeleton className="h-5 w-12 rounded-md" />
          </span>
        </div>
      ))}
      <div className="border-border/40 mt-2 flex items-center justify-between border-t pt-2">
        <L w="w-24" />
        <Skeleton className="h-3 w-12" />
      </div>
    </div>
  );
}

/** `ServerInfoClient`: Build, Dependencies, Runtime, Probes, OCR cards. */
export function ServerInfoSkeleton({
  label = "Loading server info…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion
      label={label}
      className="grid grid-cols-1 gap-4 lg:grid-cols-2"
    >
      <InfoCard title="w-10">
        <KvRows rows={["w-14", "w-16", "w-8", "w-12", "w-20"]} />
      </InfoCard>
      <InfoCard title="w-24">
        <KvRows badge rows={["w-16", "w-10"]} />
      </InfoCard>
      <InfoCard title="w-16">
        <KvRows badge rows={["w-28", "w-32"]} />
      </InfoCard>
      <InfoCard title="w-32">
        <KvRows rows={["w-24", "w-28", "w-32"]} />
      </InfoCard>
      <InfoCard title="w-24">
        <OcrModelsBodySkeleton />
      </InfoCard>
    </LoadingRegion>
  );
}

/** One server setting card: title + hint, a field, Save. */
function SettingCard() {
  return (
    <Card>
      <TitleHeader title="w-28" desc={["w-3/4"]} />
      <CardContent className="space-y-4">
        <Field label="w-32" hint="w-64" className="max-w-sm" />
        <div className="flex justify-end">
          <Btn w="w-16" />
        </div>
      </CardContent>
    </Card>
  );
}

/** `ServerSettingsCards`: the two-up hardening / diagnostics row, then
 *  the full-width settings cards. */
export function ServerSettingsSkeleton({
  label = "Loading server settings…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <SettingCard />
        <SettingCard />
      </div>
      <SettingCard />
      <SettingCard />
      <SettingCard />
    </LoadingRegion>
  );
}

/** `/admin/server`: info cards under the header, then the settings. */
export function ServerPageSkeleton() {
  return (
    <div className="space-y-6">
      <div>
        <AdminHeaderSkeleton title={11} description={103} />
        <ServerInfoSkeleton />
      </div>
      <ServerSettingsSkeleton label="" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Starter lists (/admin/starter-lists)
// ---------------------------------------------------------------------------

/** Starter-list rows: title + publisher badge, reason, coverage + path,
 *  and the Show switch. */
export function StarterRowsSkeleton({
  rows = 6,
  label = "Loading starter lists…",
}: {
  rows?: number;
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <ul className="divide-border/60 divide-y rounded-md border">
        {Array.from({ length: rows }, (_, i) => (
          <li key={i} className="flex items-start gap-3 p-3">
            <div className="min-w-0 flex-1 space-y-1">
              <div className="flex items-center gap-2">
                <L line="h-6" w={i % 2 ? "w-36" : "w-44"} />
                <Skeleton className="h-5 w-12 rounded-md" />
              </div>
              <L w={lineWidth(i)} />
              <L line="h-4" bar="h-2.5" w="w-96" />
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <Skeleton className="h-2.5 w-8" />
              <Skeleton className="h-[18px] w-8 rounded-full" />
            </div>
          </li>
        ))}
      </ul>
    </LoadingRegion>
  );
}

/** `/admin/starter-lists` (`StarterListsAdmin`): "On the shelf" list and
 *  the "Add a list" form. */
export function StarterListsSkeleton() {
  return (
    <div className="space-y-8">
      <section className="space-y-3">
        <div aria-hidden className="flex items-center justify-between gap-2">
          <L line="h-6" bar="h-4" w="w-24" />
          <Btn w="w-36" />
        </div>
        <StarterRowsSkeleton />
      </section>
      <section aria-hidden className="space-y-3">
        <L line="h-6" bar="h-4" w="w-20" />
        <div className="max-w-2xl space-y-3">
          <Field label="w-20" gap="space-y-1.5" />
          <Field label="w-10" gap="space-y-1.5" />
          <Field label="w-24" control="h-16" gap="space-y-1.5" />
          <Btn w="w-32" h="h-9" />
        </div>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Stats (/admin/stats)
// ---------------------------------------------------------------------------

/** Stats Overview tab (`StatsClient`): totals, the 14-day chart, the
 *  most-read series list. */
export function StatsOverviewSkeleton({
  label = "Loading stats…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-6">
      <StatCardRowSkeleton />
      <Card>
        <TitleHeader title="w-44" desc={["w-80"]} />
        <CardContent>
          <div className="flex h-[140px] items-end gap-2 pl-8">
            {Array.from({ length: 14 }, (_, i) => (
              <Skeleton
                key={i}
                className="flex-1 rounded-none rounded-t-sm"
                style={{
                  height: `${[8, 14, 6, 20, 10, 4, 16, 12, 6, 18, 8, 14, 30, 90][i]}%`,
                }}
              />
            ))}
          </div>
        </CardContent>
      </Card>
      <Card>
        <TitleHeader title="w-56" desc={["w-72"]} />
        <CardContent>
          <div className="divide-border divide-y">
            {Array.from({ length: 5 }, (_, i) => (
              <div
                key={i}
                className="flex items-center justify-between gap-3 py-2"
              >
                <span className="flex items-center gap-2">
                  <Skeleton className="h-3 w-4" />
                  <L w={["w-24", "w-20", "w-28", "w-16", "w-24"][i]!} />
                </span>
                <Skeleton className="h-3 w-14" />
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </LoadingRegion>
  );
}

/** `/admin/stats` (`StatsTabs`, Overview tab). */
export function StatsTabsSkeleton() {
  return (
    <LineTabsShell widths={["w-16", "w-10", "w-[5.5rem]", "w-14", "w-12"]}>
      <StatsOverviewSkeleton />
    </LineTabsShell>
  );
}

// ---------------------------------------------------------------------------
// Users (/admin/users, /admin/users/<slug>)
// ---------------------------------------------------------------------------

/** The user table (`UserTable` while loading): the "N users" count, then
 *  the email / name / role / state / last login / libraries table. */
export function UserRowsSkeleton({
  rows = 8,
  label = "Loading users…",
}: {
  rows?: number;
  label?: string;
}) {
  return (
    <LoadingRegion label={label} className="space-y-4">
      <L line="h-4" bar="h-2.5" w="w-12" />
      <TableSkeleton
        cols="minmax(0,1.4fr) minmax(0,1fr) 5rem 5rem 9rem 5rem"
        head={["w-10", "w-24", "w-8", "w-10", "w-16", "w-14"]}
        rows={rows}
        cells={(r) => [
          <Skeleton key="e" className={cn("h-3.5", r % 2 ? "w-44" : "w-36")} />,
          <Skeleton key="n" className="h-3.5 w-28" />,
          <Skeleton key="r" className="h-5 w-12 rounded-md" />,
          <Skeleton key="s" className="h-5 w-12 rounded-md" />,
          <Skeleton key="l" className="h-3 w-28" />,
          <Skeleton key="b" className="h-3 w-6" />,
        ]}
      />
    </LoadingRegion>
  );
}

/** `/admin/users` (`UserTable`): role/state chips, search + Create user,
 *  then the table. */
export function UserTableSkeleton() {
  return (
    <div className="space-y-4">
      <div aria-hidden className="flex flex-wrap items-center gap-2">
        <L line="h-4" bar="h-2.5" w="w-7" />
        {["w-8", "w-12", "w-10"].map((w, i) => (
          <Pill key={i} w={w} />
        ))}
        <L line="ml-2 h-4" bar="h-2.5" w="w-8" />
        {["w-8", "w-14", "w-16", "w-32"].map((w, i) => (
          <Pill key={i} w={w} />
        ))}
        <div className="ml-auto flex items-center gap-2">
          <Skeleton className="h-8 w-56 rounded-md" />
          <Btn w="w-[6.5rem]" />
        </div>
      </div>
      <UserRowsSkeleton />
    </div>
  );
}

/** `/admin/users/<slug>` (`UserDetail`): breadcrumb header (name + email),
 *  the Profile / Library access / Audit log / Reading tabs and the
 *  profile form with the account-state card. */
export function UserDetailSkeleton({
  label = "Loading user…",
}: {
  label?: string;
}) {
  return (
    <LoadingRegion label={label}>
      <AdminHeaderSkeleton breadcrumb title={6} description={18} />
      <div className="flex flex-col gap-2">
        <LineTabStripSkeleton widths={["w-12", "w-24", "w-16", "w-14"]} />
        <div className="space-y-6">
          <div className="space-y-3">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field label="w-24" gap="space-y-1" />
              <Field label="w-10" gap="space-y-1" />
            </div>
            <div>
              <L line="h-3.5 mb-2" bar="h-3" w="w-8" />
              <div className="flex gap-2">
                <Skeleton className="h-[22px] w-14 rounded-full" />
                <Skeleton className="h-[22px] w-12 rounded-full" />
              </div>
            </div>
            <div className="flex items-center gap-2 pt-2">
              <Btn w="w-24" />
            </div>
          </div>
          <div className="border-border bg-card space-y-2 rounded-md border p-4">
            <div className="flex items-start justify-between gap-4">
              <div>
                <L w="w-24" />
                <div className="flex items-center gap-2 pt-1">
                  <Skeleton className="h-5 w-14 rounded-md" />
                  <Skeleton className="h-3 w-64" />
                </div>
              </div>
              <Btn w="w-16" />
            </div>
            <L line="h-4" bar="h-2.5" w="w-72" />
          </div>
        </div>
      </div>
    </LoadingRegion>
  );
}
