"use client";

import type { LucideIcon } from "lucide-react";
import type * as React from "react";

import type {
  LogWidgetKind,
  LogWidgetView,
  ReadingLogEventKind,
  ReadingStatsRange,
} from "@/lib/api/types";

/** Page-level filter state every widget can read but only the chrono
 *  feed (and series-finishes / recent-bookmarks) actually applies.
 *  Carried from `<LogHeader>` through `<ReadingLogPage>`. */
export type LogScope = {
  range: ReadingStatsRange;
  kinds: ReadingLogEventKind[];
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

export type ChronoFeedConfig = {
  group_by_day: boolean;
  /** Empty = all kinds; otherwise narrows the feed. Page-level
   *  filter still applies on top. */
  default_kinds: ReadingLogEventKind[];
};

export type StatsHeroConfig = {
  /** Subset of ["issues","hours","streak","pages","pace_spp"].
   *  Empty = "issues","hours","streak" (the M2 defaults). */
  metrics: StatsHeroMetric[];
};
export type StatsHeroMetric =
  | "issues"
  | "hours"
  | "streak"
  | "pages"
  | "pace_spp";

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
