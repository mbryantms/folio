import type * as React from "react";

import { settingsNav } from "@/components/admin/nav";
import {
  BOARD_CONTAINER,
  ROW_CELL,
  ROW_GRID,
  ROW_PAD,
} from "@/components/saved-views/board-rows";
import {
  LineTabStripSkeleton,
  TextLineSkeleton,
  lineWidth,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * Settings loading shapes: the body of each settings page, built from the
 * real components' markup (`SettingsSection` cards, `SegmentedControl`
 * groups, toggle rows, list rows). Shared by the `(settings)` route
 * fallback (`SettingsRouteSkeleton`, which adds the `PageHeader`) and the
 * client components' own loading branches, so the handoff from the route
 * skeleton to the in-page one holds still.
 *
 * Static, server-safe trees. Wrap them in `LoadingRegion` (which hides the
 * boxes from assistive tech and speaks one label).
 */

/* ───────────────────────── primitives ───────────────────────── */

/** `SettingsSection`: `text-base` heading + `text-sm` description over the
 *  bordered `p-5` card. `description` takes one width per line. */
export function SettingsSectionSkeleton({
  title = "w-32",
  description = ["w-96"],
  className,
  children,
}: {
  title?: string;
  description?: readonly string[];
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <section className={cn("space-y-3", className)}>
      <div>
        <TextLineSkeleton lineClassName="h-6" className={cn("h-4", title)} />
        {description.length ? (
          <div className="mt-1">
            {description.map((w, i) => (
              <TextLineSkeleton key={i} className={cn("max-w-full", w)} />
            ))}
          </div>
        ) : null}
      </div>
      <div className="border-border bg-card rounded-lg border p-5 shadow-sm">
        {children}
      </div>
    </section>
  );
}

/** `SegmentedControl`: the bordered `p-0.5` group of `px-3 py-1.5` pills
 *  (38px tall). One width per option label. */
export function SegmentedSkeleton({
  widths,
  className,
}: {
  widths: readonly string[];
  className?: string;
}) {
  return (
    <div
      className={cn(
        "border-input bg-background inline-flex max-w-full rounded-md border p-0.5",
        className,
      )}
    >
      {widths.map((w, i) => (
        <div key={i} className="flex h-8 items-center px-3">
          <Skeleton className={cn("h-3", w)} />
        </div>
      ))}
    </div>
  );
}

/** A form field: `text-sm` label, `h-9` input, optional `text-xs` hint. */
export function FieldSkeleton({
  label = "w-24",
  hint,
}: {
  label?: string;
  hint?: string;
}) {
  return (
    <div className="space-y-2">
      <TextLineSkeleton lineClassName="h-3.5" className={cn("h-3", label)} />
      <Skeleton className="h-9 w-full rounded-md" />
      {hint ? (
        <TextLineSkeleton lineClassName="h-4" className={cn("h-2.5", hint)} />
      ) : null}
    </div>
  );
}

/** Title + description on the left, a switch (or button) on the right —
 *  the settings toggle row (`PrivacyControls` `Row`, Sidebar prefs, Page
 *  strip / Spreads). */
export function ToggleRowSkeleton({
  title = "w-40",
  lines = ["w-[26rem]", "w-80"],
  right = "switch",
}: {
  title?: string;
  lines?: readonly string[];
  right?: "switch" | "button";
}) {
  return (
    <div className="flex items-start justify-between gap-6">
      <div className="min-w-0 flex-1 space-y-0.5">
        <TextLineSkeleton className={title} />
        <div className="max-w-prose">
          {lines.map((w, i) => (
            <TextLineSkeleton key={i} className={cn("max-w-full", w)} />
          ))}
        </div>
      </div>
      <div className="shrink-0 pt-1">
        {right === "switch" ? (
          <Skeleton className="h-[18px] w-8 rounded-full" />
        ) : (
          <Skeleton className="h-8 w-28 rounded-md" />
        )}
      </div>
    </div>
  );
}

/** A `size-7` icon button / drag grip cell. */
function IconCell({ hidden = false }: { hidden?: boolean }) {
  return (
    <div
      className={cn(
        "flex size-7 shrink-0 items-center justify-center",
        hidden && "max-sm:hidden",
      )}
    >
      <Skeleton className="size-3.5 rounded" />
    </div>
  );
}

/* ───────────────────────── /settings ───────────────────────── */

/** `SettingsIndex` body: per section, the uppercase label over a grid of
 *  icon + label link tiles (`settingsNav` drives the counts and widths). */
export function SettingsIndexBodySkeleton() {
  return (
    <>
      {settingsNav("").map((section) => (
        <section key={section.label} className="space-y-3">
          <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-14" />
          <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
            {section.items.map((item) => (
              <li key={item.href}>
                <div className="border-border/60 flex items-center gap-3 rounded-lg border p-4">
                  <Skeleton className="size-5 shrink-0 rounded" />
                  <div className="flex h-6 min-w-0 items-center">
                    <Skeleton
                      className="h-3.5 max-w-full"
                      style={{ width: `${item.label.length * 0.55}rem` }}
                    />
                  </div>
                </div>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </>
  );
}

/* ───────────────────────── /settings/account ───────────────────────── */

/** `AccountForm`: Profile (two fields + Save), Sidebar (one toggle), and
 *  Change password (three fields) — the cards above the fold. */
export function AccountBodySkeleton() {
  return (
    <div className="space-y-6">
      <SettingsSectionSkeleton title="w-16" description={["w-[30rem]"]}>
        <div className="space-y-5">
          <FieldSkeleton label="w-24" />
          <FieldSkeleton label="w-10" hint="w-64" />
          <div className="flex justify-end">
            <Skeleton className="h-9 w-24 rounded-md" />
          </div>
        </div>
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-16" description={["w-72"]}>
        <ToggleRowSkeleton
          title="w-48"
          lines={["w-[32rem]", "w-[30rem]", "w-8"]}
        />
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-36" description={["w-[32rem]"]}>
        <div className="space-y-5">
          <FieldSkeleton label="w-28" />
          <FieldSkeleton label="w-24" hint="w-32" />
          <FieldSkeleton label="w-36" />
        </div>
      </SettingsSectionSkeleton>
    </div>
  );
}

/* ───────────────────────── /settings/privacy ───────────────────────── */

/** "Reading activity": `PrivacyControls`' three rows (two switches +
 *  Delete history). */
export function PrivacyBodySkeleton() {
  return (
    <SettingsSectionSkeleton title="w-32" description={["w-[30rem]"]}>
      <div className="space-y-4">
        <ToggleRowSkeleton title="w-44" lines={["w-[32rem]", "w-48"]} />
        <ToggleRowSkeleton title="w-56" lines={["w-[32rem]", "w-[28rem]"]} />
        <ToggleRowSkeleton
          title="w-40"
          lines={["w-[31rem]", "w-72"]}
          right="button"
        />
      </div>
    </SettingsSectionSkeleton>
  );
}

/* ───────────────────────── /settings/api-tokens ───────────────────────── */

/** `AppPasswordsCard`: Generate (label + input + Generate, the two scope
 *  radios), Existing passwords, then OPDS readers. */
export function ApiTokensBodySkeleton() {
  return (
    <div className="space-y-6">
      <SettingsSectionSkeleton title="w-44" description={["w-[46rem]"]}>
        <div className="space-y-3">
          <div className="flex items-end gap-3">
            <div className="flex-1 space-y-1.5">
              <TextLineSkeleton lineClassName="h-3.5" className="h-3 w-10" />
              <Skeleton className="h-9 w-full rounded-md" />
            </div>
            <Skeleton className="h-9 w-20 rounded-md" />
          </div>
          <div className="space-y-1.5">
            <TextLineSkeleton lineClassName="h-3.5" className="h-3 w-12" />
            <div className="flex flex-col gap-1.5">
              {[["w-[34rem]"], ["w-full", "w-56"]].map((lines, i) => (
                <div key={i} className="flex items-start gap-2">
                  <Skeleton className="mt-1 size-3.5 shrink-0 rounded-full" />
                  <div className="min-w-0 flex-1">
                    {lines.map((w, j) => (
                      <TextLineSkeleton
                        key={j}
                        className={cn("max-w-full", w)}
                      />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-40" description={["w-[40rem]"]}>
        <TextLineSkeleton className="w-64" />
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-28" description={["w-full", "w-16"]}>
        <div className="space-y-3">
          <div className="space-y-1">
            <TextLineSkeleton lineClassName="h-3.5" className="h-2.5 w-28" />
            <Skeleton className="h-10 w-full rounded-md" />
          </div>
          <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-full" />
          <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-40" />
        </div>
      </SettingsSectionSkeleton>
    </div>
  );
}

/* ───────────────────────── /settings/downloads ───────────────────────── */

/** "On this device": storage line + meter + note, then the list (shown as
 *  the empty-state box, the common case on a fresh device). */
export function DownloadsBodySkeleton() {
  return (
    <SettingsSectionSkeleton title="w-28" description={["w-[38rem]"]}>
      <div className="space-y-4">
        <div className="space-y-2">
          <TextLineSkeleton className="w-36" />
          <Skeleton className="h-1.5 w-full rounded-full" />
          <TextLineSkeleton
            lineClassName="h-4"
            className="h-2.5 w-[34rem] max-w-full"
          />
        </div>
        <div className="border-border/60 flex flex-col items-center gap-2 rounded-lg border px-6 py-8">
          <Skeleton className="h-12 w-20 rounded-lg" />
          <TextLineSkeleton className="mt-1 w-28" />
          <TextLineSkeleton className="w-96 max-w-full" />
          <TextLineSkeleton className="w-80 max-w-full" />
        </div>
      </div>
    </SettingsSectionSkeleton>
  );
}

/* ───────────────────────── /settings/keybinds ───────────────────────── */

function KeybindRowsSkeleton({
  rows,
  offset = 0,
  directionAware = 0,
}: {
  rows: number;
  offset?: number;
  /** Leading rows carrying the italic "Direction-aware" note. */
  directionAware?: number;
}) {
  return (
    <ul className="divide-border divide-y">
      {Array.from({ length: rows }, (_, i) => (
        <li key={i} className="flex items-center justify-between gap-4 py-3">
          <div className="min-w-0">
            <TextLineSkeleton className={i % 2 ? "w-24" : "w-28"} />
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-20" />
            {i < directionAware ? (
              <TextLineSkeleton
                lineClassName="mt-0.5 h-4"
                className="h-2.5 w-72 max-w-full"
              />
            ) : null}
          </div>
          <Skeleton
            className={cn(
              "h-8 shrink-0 rounded",
              (i + offset) % 3 === 0 ? "w-14" : "w-10",
            )}
          />
        </li>
      ))}
    </ul>
  );
}

/** `KeybindEditor`: Global hotkeys (three rows) and Reader hotkeys (the
 *  first rows of the long list; arrows carry the direction note). Sibling
 *  sections, as the editor renders them in a fragment. */
export function KeybindsBodySkeleton() {
  return (
    <>
      <SettingsSectionSkeleton title="w-28" description={["w-96"]}>
        <KeybindRowsSkeleton rows={3} />
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-28" description={["w-[30rem]"]}>
        <KeybindRowsSkeleton rows={8} offset={1} directionAware={2} />
      </SettingsSectionSkeleton>
    </>
  );
}

/* ───────────────────────── /settings/pages ───────────────────────── */

/** `PageRailsList` rows: grip, position, name (+ description), kind badge
 *  and the remove button, under the card's hairline. */
export function RailRowsSkeleton({ rows = 8 }: { rows?: number }) {
  return (
    <ul className="border-border/60 divide-border/60 divide-y border-t">
      {Array.from({ length: rows }, (_, i) => (
        <li key={i} className="flex items-center gap-3 px-3 py-2">
          <IconCell />
          <div className="flex w-5 shrink-0 justify-end">
            <Skeleton className="h-2.5 w-2" />
          </div>
          <div className="min-w-0 flex-1">
            <TextLineSkeleton className={cn("max-w-64", lineWidth(i))} />
            {i < 2 ? (
              <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-48" />
            ) : null}
          </div>
          <Skeleton className="h-4 w-14 shrink-0 rounded-full max-sm:hidden" />
          <IconCell />
        </li>
      ))}
    </ul>
  );
}

/** `PagesManager`: intro + New page, the max-rails card, the line tab
 *  strip of pages, and the first page's card (header, "N / M rails" +
 *  Add view, rail rows). */
export function PagesManagerSkeleton() {
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <TextLineSkeleton className="w-[30rem] max-w-full" />
        <Skeleton className="h-9 w-28 rounded-md" />
      </div>
      <div className="border-border/60 bg-card/40 flex flex-wrap items-center gap-3 rounded-md border p-3">
        <div className="min-w-0 flex-1">
          <TextLineSkeleton className="w-40" />
          <TextLineSkeleton
            lineClassName="mt-0.5 h-4"
            className="h-2.5 w-[26rem] max-w-full"
          />
        </div>
        <Skeleton className="h-9 w-20 rounded-md" />
      </div>
      <div className="flex flex-col gap-2">
        <LineTabStripSkeleton widths={["w-24", "w-72", "w-80"]} />
        <div className="border-border/60 rounded-lg border">
          <div className="border-border/60 flex items-start justify-between gap-3 border-b p-4">
            <div className="flex h-6 items-center gap-2">
              <Skeleton className="h-4 w-16" />
              <Skeleton className="h-3 w-10" />
            </div>
            <Skeleton className="size-8 shrink-0 rounded-md" />
          </div>
          <div className="flex items-center justify-between gap-2 p-3">
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-16" />
            <Skeleton className="h-8 w-24 rounded-md" />
          </div>
          <RailRowsSkeleton />
        </div>
      </div>
    </div>
  );
}

/** `HiddenRailItems` section (below Pages). */
export function HiddenRailItemsSkeleton() {
  return (
    <SettingsSectionSkeleton title="w-28" description={["w-[36rem]"]}>
      <ul className="divide-border/60 divide-y">
        {Array.from({ length: 2 }, (_, i) => (
          <li
            key={i}
            className="flex items-center gap-3 py-2 first:pt-0 last:pb-0"
          >
            <Skeleton className="aspect-[2/3] w-10 shrink-0" />
            <div className="min-w-0 flex-1">
              <TextLineSkeleton className={lineWidth(i)} />
              <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-40" />
            </div>
            <Skeleton className="h-8 w-20 rounded-md" />
          </li>
        ))}
      </ul>
    </SettingsSectionSkeleton>
  );
}

/* ───────────────────────── /settings/reading ───────────────────────── */

/** `ReadingPrefs`: the sticky preview column (first on phones) beside the
 *  stacked control cards. */
export function ReadingBodySkeleton() {
  return (
    <div className="lg:grid lg:grid-cols-[minmax(0,1fr)_18rem] lg:items-start lg:gap-8">
      <div className="mb-6 space-y-2 lg:order-2 lg:mb-0">
        <div className="flex h-4 items-center justify-between">
          <Skeleton className="h-2.5 w-14" />
          <Skeleton className="h-3.5 w-8 rounded" />
        </div>
        <Skeleton className="aspect-[16/10] w-full rounded-lg" />
        <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-48" />
      </div>
      <div className="min-w-0 space-y-6">
        <SettingsSectionSkeleton title="w-32" description={["w-[30rem]"]}>
          <SegmentedSkeleton widths={["w-8", "w-20", "w-20"]} />
        </SettingsSectionSkeleton>
        <SettingsSectionSkeleton title="w-16" description={["w-64"]}>
          <SegmentedSkeleton widths={["w-8", "w-14", "w-16", "w-14", "w-16"]} />
        </SettingsSectionSkeleton>
        <SettingsSectionSkeleton title="w-20" description={["w-[30rem]"]}>
          <SegmentedSkeleton widths={["w-8", "w-11", "w-12", "w-14"]} />
        </SettingsSectionSkeleton>
        <SettingsSectionSkeleton
          title="w-36"
          description={["w-[36rem]", "w-56"]}
        >
          <SegmentedSkeleton widths={["w-8", "w-7", "w-5"]} />
        </SettingsSectionSkeleton>
        <SettingsSectionSkeleton title="w-20" description={["w-80"]}>
          <ToggleRowSkeleton title="w-36" lines={["w-80"]} />
        </SettingsSectionSkeleton>
      </div>
    </div>
  );
}

/* ───────────────────────── /settings/theme ───────────────────────── */

/** `ThemePicker`: Theme, Accent (chips), Density, Corners, Motion. */
export function ThemeBodySkeleton() {
  return (
    <div className="space-y-6">
      <SettingsSectionSkeleton title="w-14" description={["w-[46rem]"]}>
        <SegmentedSkeleton widths={["w-12", "w-8", "w-9", "w-11"]} />
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-24" description={["w-96"]}>
        <div className="flex flex-wrap gap-3">
          {["w-[4.5rem]", "w-16", "w-20", "w-16"].map((w, i) => (
            <Skeleton key={i} className={cn("h-[34px] rounded-md", w)} />
          ))}
        </div>
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-16" description={["w-[30rem]"]}>
        <SegmentedSkeleton widths={["w-20", "w-14"]} />
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-16" description={["w-[30rem]"]}>
        <SegmentedSkeleton widths={["w-10", "w-12", "w-10"]} />
      </SettingsSectionSkeleton>
      <SettingsSectionSkeleton title="w-14" description={["w-[32rem]"]}>
        <SegmentedSkeleton widths={["w-24", "w-14", "w-8"]} />
      </SettingsSectionSkeleton>
    </div>
  );
}

/* ───────────────────────── /settings/sidebar ───────────────────────── */

/** Default sidebar layout: headers (`H`) over their rows. */
const SIDEBAR_ROWS = "HbbbbbbHlllHpp";

/** `NavigationManager`: the right-aligned Add / Header / Spacer / Reset
 *  toolbar over the bordered list of rows (grip, icon, label, kind chip,
 *  remove). */
export function SidebarLayoutSkeleton() {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Skeleton className="h-8 w-[4.25rem] rounded-md" />
        <Skeleton className="h-8 w-[5.25rem] rounded-md" />
        <Skeleton className="h-8 w-20 rounded-md" />
        <Skeleton className="h-8 w-[4.75rem] rounded-md" />
      </div>
      <ul className="border-border/60 divide-border/60 divide-y overflow-hidden rounded-lg border">
        {SIDEBAR_ROWS.split("").map((kind, i) => {
          const header = kind === "H";
          return (
            <li
              key={i}
              className={cn(
                "flex items-center gap-3 px-3 py-2",
                header && "bg-muted/30",
              )}
            >
              <IconCell hidden />
              <Skeleton className="size-4 shrink-0 rounded" />
              <div className="min-w-0 flex-1">
                <Skeleton
                  className={cn(
                    "h-3.5",
                    header ? "w-20" : ["w-12", "w-20", "w-16", "w-24"][i % 4],
                  )}
                />
              </div>
              <Skeleton className="h-3 w-12 shrink-0 max-sm:hidden" />
              <IconCell />
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/* ───────────────────────── /settings/views ───────────────────────── */

/** Compact `ViewCard`s in the board's compact grid: icon + ⋯, name,
 *  one-line description, the Built-in / pin badge row. */
export function ViewCardsSkeleton({ count = 12 }: { count?: number }) {
  return (
    <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5 xl:grid-cols-6">
      {Array.from({ length: count }, (_, i) => (
        <li key={i}>
          <div className="border-border/60 bg-card flex h-full flex-col gap-2 rounded-lg border p-3">
            <div className="flex items-start justify-between gap-2">
              <Skeleton className="size-5 rounded" />
              <Skeleton className="h-4 w-1.5" />
            </div>
            <div className="space-y-0.5">
              <TextLineSkeleton className={lineWidth(i)} />
              <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-4/5" />
            </div>
            <div className="mt-auto flex gap-1.5 pt-1">
              <Skeleton className="h-5 w-16 rounded-full" />
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}

/** `GroupedViewsBoard`'s rows presentation (the reading lists table):
 *  the bordered table, its column header, one folder row and list rows
 *  on the shared `ROW_GRID` — a small cover, the name and description,
 *  the read bar and the In library column. It carries its own
 *  `@container/board`, so the route fallback (outside the board) picks
 *  the same tier the board will. */
export function ViewRowsSkeleton({ count = 8 }: { count?: number }) {
  return (
    <div className={BOARD_CONTAINER}>
      <div className="border-border/60 bg-card/40 rounded-lg border">
        <div
          className={cn(
            ROW_GRID,
            "border-border/60 hidden border-b px-2 py-1.5 @min-[30rem]/board:grid @min-[30rem]/board:px-3",
          )}
        >
          <div className="@min-[30rem]/board:col-span-2 @min-[30rem]/board:col-start-2 @min-[30rem]/board:row-start-1">
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-10" />
          </div>
          <div className={ROW_CELL.stat}>
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-8" />
          </div>
          <div className={ROW_CELL.stat2}>
            <TextLineSkeleton lineClassName="h-4" className="h-2.5 w-14" />
          </div>
        </div>
        {Array.from({ length: count }, (_, i) => {
          const folder = i === 0;
          return (
            <div
              key={i}
              className={cn(
                ROW_GRID,
                ROW_PAD,
                i > 0 && "border-border/50 border-t",
                folder && "bg-muted/30",
              )}
              style={
                {
                  "--row-depth": folder ? 0 : i < 4 ? 1 : 0,
                } as React.CSSProperties
              }
            >
              <div className={ROW_CELL.grip} />
              <div className={ROW_CELL.lead}>
                {folder ? (
                  <Skeleton className="size-4 rounded" />
                ) : (
                  <Skeleton className="aspect-[2/3] w-12 rounded-md @min-[30rem]/board:w-7" />
                )}
              </div>
              <div className={cn(ROW_CELL.main, "space-y-0.5")}>
                <TextLineSkeleton className={cn("max-w-full", lineWidth(i))} />
                {folder ? null : (
                  <TextLineSkeleton
                    lineClassName="h-4"
                    className="h-2.5 w-4/5 max-w-full"
                  />
                )}
              </div>
              <div className={cn(ROW_CELL.stat, "space-y-1.5")}>
                <Skeleton className="h-1.5 w-full rounded-full" />
                <Skeleton className="h-2.5 w-20" />
              </div>
              <div className={ROW_CELL.stat2}>
                <Skeleton className="h-2.5 w-24" />
              </div>
              <div className={ROW_CELL.menu}>
                <IconCell />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** The rows board's density toggle (two 36px segments; wide tier only,
 *  read from the board's container). */
export function DensityToggleSkeleton() {
  return (
    <Skeleton className="hidden h-9 w-[4.5rem] rounded-md @min-[42rem]/board:block" />
  );
}

/** The board toolbar's search / sort / New group controls (shown once the
 *  board has cards; the primary action sits after them). `density` adds
 *  the rows board's density toggle between sort and New group. */
export function BoardToolbarControlsSkeleton({
  density = false,
}: {
  density?: boolean;
}) {
  return (
    <>
      <Skeleton className="h-9 w-full rounded-md max-sm:basis-full sm:w-56" />
      <Skeleton className="h-9 w-[11.5rem] rounded-md" />
      {density ? <DensityToggleSkeleton /> : null}
      <Skeleton className="h-9 w-28 rounded-md" />
    </>
  );
}

/** `ViewsIndex` below its header: the line tab strip (label + count), the
 *  Filter views toolbar and the card grid. */
export function ViewsTabsSkeleton() {
  return (
    <div className="flex flex-col gap-2 space-y-4">
      <LineTabStripSkeleton widths={["w-24", "w-28", "w-24"]} />
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <TextLineSkeleton className="w-48" />
          <div className="flex flex-wrap items-center gap-2 max-sm:w-full">
            <BoardToolbarControlsSkeleton />
            <Skeleton className="h-9 w-32 rounded-md" />
          </div>
        </div>
        <ViewCardsSkeleton />
      </div>
    </div>
  );
}
