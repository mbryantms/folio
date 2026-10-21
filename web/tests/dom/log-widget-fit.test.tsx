// @vitest-environment jsdom
/**
 * Reading-log widgets must fit their card at every width. jsdom has no
 * layout engine, so this pins the CSS decisions that keep them inside:
 *
 *  - the widget grid splits into two columns on a *container* query
 *    (`@3xl`), not the `md:` viewport breakpoint that produced two
 *    ~250px columns beside the sidebar;
 *  - "At a glance" tiles auto-fit and wrap instead of a fixed
 *    `repeat(N, 1fr)` row that ran SEC/PAGE out of the card;
 *  - the time-of-day legend sits beside the donut only on `@md` and its
 *    rows never wrap;
 *  - the heatmap renders at natural size inside a horizontal scroller
 *    (scrolled to the latest week) rather than scaling to a 3px strip.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ActivityHeatmap } from "@/components/activity/ActivityHeatmap";
import { TimeOfDayDonut } from "@/components/activity/TimeOfDayDonut";
import { StatsHero } from "@/components/log/widgets/StatsHero";
import type { StatsHeroConfig } from "@/components/log/widgets/types";
import type { LogWidgetView, ReadingStatsView } from "@/lib/api/types";

vi.mock("@/lib/api/mutations", () => ({
  useRemoveLogWidget: () => ({ mutate: vi.fn(), isPending: false }),
}));

const statsData = {
  totals: {
    distinct_issues: 27,
    distinct_pages_read: 611,
    active_ms: 16_560_000,
    current_streak: 6,
  },
  pace_series: [{ sec_per_page: 21.4 }],
} as unknown as ReadingStatsView;

vi.mock("@/lib/api/queries", () => ({
  useReadingStats: () => ({ data: statsData, isLoading: false }),
}));

const widget = {
  id: "w1",
  kind: "stats_hero",
  position: 0,
  config: { metrics: ["streak", "hours", "issues", "pace_spp", "pages"] },
} as unknown as LogWidgetView & { config: StatsHeroConfig };

describe("reading-log widgets fit their card", () => {
  it("At a glance tiles auto-fit and wrap; values truncate instead of overflowing", () => {
    const { container } = render(
      <StatsHero widget={widget} scope={{ range: "30d" }} />,
    );
    const grid = container.querySelector(".grid");
    expect(grid?.className).toMatch(/repeat\(auto-fit,minmax\(/);
    expect(grid?.getAttribute("style")).toBeNull();
    expect(grid?.children).toHaveLength(5);
    for (const tile of grid!.children) {
      expect(tile.className).toMatch(/\bmin-w-0\b/);
      expect(tile.querySelector(".text-lg")?.className).toMatch(/\btruncate\b/);
    }
    expect(screen.getByText("21.4s")).toBeTruthy();
  });

  it("time-of-day legend goes beside the donut on a container query and never wraps", () => {
    const { container } = render(
      <TimeOfDayDonut
        data={{
          morning: { active_ms: 1_113_000, sessions: 2 },
          afternoon: { active_ms: 2_671_000, sessions: 3 },
          evening: { active_ms: 3_584_000, sessions: 4 },
          night: { active_ms: 9_000_000, sessions: 5 },
        }}
      />,
    );
    const root = container.firstElementChild!;
    expect(root.className).toMatch(/@container/);
    const grid = root.firstElementChild!;
    expect(grid.className).toMatch(/@md:grid-cols-2/);
    expect(grid.className).not.toMatch(/\bsm:grid-cols-2\b/);
    const rows = container.querySelectorAll("li");
    expect(rows).toHaveLength(4);
    for (const li of rows) {
      const label = li.children[0]!;
      const value = li.children[1]!;
      expect(label.className).toMatch(/\bmin-w-0\b/);
      expect(value.className).toMatch(/\bwhitespace-nowrap\b/);
      expect(value.className).toMatch(/\bshrink-0\b/);
    }
  });

  it("heatmap renders at natural size inside a horizontal scroller", () => {
    const { container } = render(
      <ActivityHeatmap
        perDay={[
          { date: "2026-10-20", active_ms: 600_000, sessions: 1, pages: 20 },
        ]}
        today={new Date("2026-10-21T12:00:00")}
        weeks={52}
      />,
    );
    const svg = container.querySelector("svg")!;
    const scroller = svg.parentElement!;
    expect(scroller.className).toMatch(/\boverflow-x-auto\b/);
    // Natural pixel width, no `max-width: 100%` scale-down.
    expect(Number(svg.getAttribute("width"))).toBeGreaterThan(700);
    expect(svg.getAttribute("style")).toBeNull();
    // Legend stays outside the scroller so it never scrolls away.
    expect(container.querySelector("figcaption")?.parentElement).toBe(
      scroller.parentElement,
    );
  });
});
