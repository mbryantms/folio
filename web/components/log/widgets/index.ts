"use client";

import {
  BookOpen,
  Building2,
  CalendarHeart,
  Clock,
  Compass,
  FlameKindling,
  LineChart,
  ListChecks,
  ScrollText,
  StickyNote,
  Users,
} from "lucide-react";

import { ChronoFeed } from "./ChronoFeed";
import { CurrentlyReading } from "./CurrentlyReading";
import { Heatmap } from "./Heatmap";
import { Note } from "./Note";
import { PaceChartWidget } from "./PaceChartWidget";
import { RecentBookmarks } from "./RecentBookmarks";
import { SeriesFinishes } from "./SeriesFinishes";
import { StatsHero } from "./StatsHero";
import { TimeOfDay } from "./TimeOfDay";
import { TopCreators } from "./TopCreators";
import { TopImprints } from "./TopImprints";
import { TopPublishers } from "./TopPublishers";
import type {
  ChronoFeedConfig,
  CurrentlyReadingConfig,
  HeatmapConfig,
  LogWidgetDef,
  NoteConfig,
  PaceChartConfig,
  RankingConfig,
  RecentBookmarksConfig,
  StatsHeroConfig,
  TopCreatorsConfig,
} from "./types";
import type { LogWidgetKind } from "@/lib/api/types";

/** Renderer registry. The page resolves each `LogWidgetView.kind` to
 *  an entry here; adding a new kind = one line plus a renderer file
 *  plus a server-side entry in `WIDGET_KINDS` + a config struct.
 *
 *  Cast to `LogWidgetDef<Record<string, unknown>>` keeps the registry
 *  homogeneous after the per-kind generic narrowing; the renderers
 *  themselves stay strictly typed against their config struct. */
export const WIDGET_REGISTRY: Record<
  LogWidgetKind,
  LogWidgetDef<Record<string, unknown>>
> = {
  chrono_feed: {
    kind: "chrono_feed",
    displayName: "Activity feed",
    description:
      "Reverse-chronological list of issues read, sessions, series finishes, and markers.",
    Icon: ScrollText,
    size: "full",
    defaultConfig: {
      group_by_day: true,
      default_kinds: [],
    } satisfies ChronoFeedConfig,
    Component: ChronoFeed as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  stats_hero: {
    kind: "stats_hero",
    displayName: "At a glance",
    description: "Three-tile summary of issues, hours, and streak.",
    Icon: FlameKindling,
    size: "half",
    defaultConfig: { metrics: [] } satisfies StatsHeroConfig,
    Component: StatsHero as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  heatmap: {
    kind: "heatmap",
    displayName: "Reading heatmap",
    description: "Year-grid of daily reading time.",
    Icon: CalendarHeart,
    size: "half",
    defaultConfig: { weeks: 52 } satisfies HeatmapConfig,
    Component: Heatmap as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  top_creators: {
    kind: "top_creators",
    displayName: "Top creators",
    description: "Top writers, pencillers, or any other credited role.",
    Icon: Users,
    size: "half",
    defaultConfig: {
      role: "writer",
      range: "30d",
      limit: 5,
    } satisfies TopCreatorsConfig,
    Component: TopCreators as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  top_publishers: {
    kind: "top_publishers",
    displayName: "Top publishers",
    description: "Most-read publishers, ranked by reading time.",
    Icon: Building2,
    size: "half",
    defaultConfig: { range: "30d", limit: 5 } satisfies RankingConfig,
    Component: TopPublishers as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  top_imprints: {
    kind: "top_imprints",
    displayName: "Top imprints",
    description: "Most-read imprints, ranked by reading time.",
    Icon: Building2,
    size: "half",
    defaultConfig: { range: "30d", limit: 5 } satisfies RankingConfig,
    Component: TopImprints as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  series_finishes: {
    kind: "series_finishes",
    displayName: "Series finished",
    description: "Series you completed in the selected window.",
    Icon: ListChecks,
    size: "half",
    defaultConfig: { range: "30d", limit: 5 } satisfies RankingConfig,
    Component: SeriesFinishes as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  pace_chart: {
    kind: "pace_chart",
    displayName: "Reading pace",
    description: "Seconds-per-page over time with a moving average.",
    Icon: LineChart,
    size: "half",
    defaultConfig: { range: "30d" } satisfies PaceChartConfig,
    Component: PaceChartWidget as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  time_of_day: {
    kind: "time_of_day",
    displayName: "When you read",
    description: "Morning / afternoon / evening / night breakdown.",
    Icon: Clock,
    size: "half",
    defaultConfig: {},
    Component: TimeOfDay as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  recent_bookmarks: {
    kind: "recent_bookmarks",
    displayName: "Recent bookmarks",
    description: "Latest bookmarks, notes, and highlights you've saved.",
    Icon: BookOpen,
    size: "half",
    defaultConfig: {
      limit: 5,
      kinds: [],
    } satisfies RecentBookmarksConfig,
    Component: RecentBookmarks as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  currently_reading: {
    kind: "currently_reading",
    displayName: "Currently reading",
    description: "Issues you've started but not finished.",
    Icon: Compass,
    size: "half",
    defaultConfig: { limit: 5 } satisfies CurrentlyReadingConfig,
    Component: CurrentlyReading as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
  note: {
    kind: "note",
    displayName: "Note",
    description: "Free-form text you pin to your log.",
    Icon: StickyNote,
    size: "half",
    defaultConfig: { body: "" } satisfies NoteConfig,
    Component: Note as unknown as LogWidgetDef<
      Record<string, unknown>
    >["Component"],
  },
};

/** Stable ordering for the "Add widget" menu — separates feed-y
 *  kinds from analytics from extras. */
export const WIDGET_KIND_ORDER: LogWidgetKind[] = [
  "chrono_feed",
  "recent_bookmarks",
  "currently_reading",
  "series_finishes",
  "stats_hero",
  "heatmap",
  "pace_chart",
  "time_of_day",
  "top_creators",
  "top_publishers",
  "top_imprints",
  "note",
];
