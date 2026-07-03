"use client";

import type { LucideIcon } from "lucide-react";
import type * as React from "react";

import type {
  LogWidgetKind,
  LogWidgetView,
  ReadingLogEventKind,
  ReadingStatsRange,
} from "@/lib/api/types";

/** Page-level state every widget can read. Only the `range` is
 *  page-controlled today; per-widget kind filtering moved into the
 *  chrono_feed widget's Configure dialog when the page-header chips
 *  proved redundant. `kinds` stays in the type as a deprecated
 *  no-op for renderers that might want to follow page-level chip
 *  state later. */
export type LogScope = {
  range: ReadingStatsRange;
  kinds?: ReadingLogEventKind[];
};

/** Props every renderer accepts. Generic over the kind-specific
 *  config shape so renderers stay typed without manual narrowing. */
export type LogWidgetProps<C extends object = Record<string, unknown>> = {
  widget: LogWidgetView & { config: C };
  scope: LogScope;
};

/** Renderer-registry entry. One per widget kind. */
export type LogWidgetDef<C extends object = Record<string, unknown>> = {
  kind: LogWidgetKind;
  displayName: string;
  description: string;
  Icon: LucideIcon;
  /** Grid footprint. `full` spans both columns on md+. */
  size: "full" | "half";
  /** Default config blob inserted when the widget is added. */
  defaultConfig: C;
  /** React renderer. */
  Component: React.ComponentType<LogWidgetProps<C>>;
  /** When true, the "Add widget" menu keeps the kind selectable even
   *  if one is already on the grid. `note` is the only kind that
   *  benefits from this today — a user might pin two notes for
   *  different topics. */
  allowMultiple?: boolean;
};

// ─── Per-kind config shapes (mirror server schemas) ───

export type ChronoFeedGroupBy = "day" | "week" | "month" | "none";

export type ChronoFeedConfig = {
  /** Top-level grouping for the rendered feed. `none` is a flat
   *  list with no headers. */
  group_by: ChronoFeedGroupBy;
  /** Grid footprint override. Defaults to `full` (the registry
   *  default); switching to `half` shrinks the widget so the user
   *  can pin another half-width widget next to it. */
  size: "full" | "half";
  /** Optional range override. Empty string falls back to the
   *  page-level `LogScope.range`. */
  range: ReadingStatsRange | "";
  /** Kind filter — empty array means all four kinds. Replaces the
   *  former page-header chip row, which was redundant once each
   *  chrono_feed widget could own its own filter. */
  default_kinds: ReadingLogEventKind[];
};

export type StatsHeroConfig = {
  /** Subset of ["issues","hours","streak","pages","pace_spp"].
   *  Empty = "issues","hours","streak" (the M2 defaults). */
  metrics: StatsHeroMetric[];
};
export type StatsHeroMetric =
  "issues" | "hours" | "streak" | "pages" | "pace_spp";

export type HeatmapConfig = {
  weeks: 4 | 8 | 12 | 26 | 52;
};

export type TopCreatorsConfig = {
  role: string;
  range: ReadingStatsRange;
  limit: number;
};

export type RankingConfig = {
  range: ReadingStatsRange;
  limit: number;
};

export type PaceChartConfig = {
  range: ReadingStatsRange;
};

export type RecentBookmarksConfig = {
  limit: number;
  /** Empty = all marker kinds. */
  kinds: string[];
};

export type CurrentlyReadingConfig = {
  limit: number;
};

export type NoteConfig = {
  body: string;
};
