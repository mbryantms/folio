import { describe, expect, it } from "vitest";

import {
  addDays,
  dayDiff,
  heatLevel,
  heatmapGrid,
  milestones,
  readingYears,
  streakRuns,
} from "@/lib/log/overview";

const d = (date: string, pages = 10) => ({ date, pages });

describe("dates", () => {
  it("adds days and diffs across months, years and DST", () => {
    expect(addDays("2026-02-27", 3)).toBe("2026-03-02");
    expect(addDays("2026-12-31", 1)).toBe("2027-01-01");
    expect(dayDiff("2026-03-28", "2026-03-30")).toBe(2);
  });
});

describe("streakRuns", () => {
  const days = [
    d("2026-10-01"),
    d("2026-10-02"),
    d("2026-10-03"),
    d("2026-10-10"),
    d("2026-10-11"),
    d("2026-11-03"),
    d("2026-11-04"),
  ];
  it("finds the longest run and the run ending today", () => {
    const r = streakRuns(days, "2026-11-04");
    expect(r.longest).toEqual({
      start: "2026-10-01",
      end: "2026-10-03",
      length: 3,
    });
    expect(r.current).toEqual({
      start: "2026-11-03",
      end: "2026-11-04",
      length: 2,
    });
  });
  it("keeps the current streak through today until a day is missed", () => {
    expect(streakRuns(days, "2026-11-05").current?.length).toBe(2);
    expect(streakRuns(days, "2026-11-06").current).toBeNull();
  });
  it("handles no reading", () => {
    expect(streakRuns([], "2026-11-04")).toEqual({
      current: null,
      longest: null,
    });
  });
});

describe("heatmapGrid", () => {
  it("ends on today's week, Monday-first, marking the streak and the future", () => {
    // 2026-11-04 is a Wednesday.
    const { columns } = heatmapGrid(
      [d("2026-11-03", 50)],
      "2026-11-04",
      { start: "2026-11-03", end: "2026-11-04", length: 2 },
      4,
    );
    expect(columns).toHaveLength(4);
    const last = columns[3]!;
    expect(last[0]!.date).toBe("2026-11-02");
    expect(last[1]).toMatchObject({
      date: "2026-11-03",
      pages: 50,
      level: 3,
      streak: true,
    });
    expect(last[3]!.future).toBe(true);
  });
  it("levels pages into five steps", () => {
    expect([0, 1, 20, 21, 46, 81].map(heatLevel)).toEqual([0, 1, 1, 2, 3, 4]);
  });
});

describe("milestones", () => {
  it("earns streak tiers from the longest run and shows the next one locked", () => {
    const days = Array.from({ length: 9 }, (_, i) =>
      d(addDays("2026-06-01", i)),
    );
    const m = milestones({
      days,
      today: "2026-11-04",
      year: 2026,
      seriesFinished: [],
    });
    expect(m.find((x) => x.id === "streak-7")).toMatchObject({
      earned: true,
      at: "2026-06-07",
    });
    expect(m.find((x) => x.id === "streak-14")?.earned).toBe(false);
    expect(m.find((x) => x.id === "streak-14")?.progress).toBeCloseTo(9 / 14);
  });

  it("counts year pages, 1,000-page months and finished series", () => {
    const days = [
      d("2026-03-01", 600),
      d("2026-03-02", 500),
      d("2025-12-01", 900),
    ];
    const m = milestones({
      days,
      today: "2026-11-04",
      year: 2026,
      seriesFinished: [
        { seriesId: "s1", name: "Saga", totalIssues: 54, at: "2026-04-01" },
      ],
    });
    expect(m.find((x) => x.id === "year-pages-1000")).toMatchObject({
      earned: true,
      at: "2026-03-02",
    });
    expect(m.find((x) => x.id === "month-1000")?.earned).toBe(true);
    expect(m.find((x) => x.id === "series-s1")).toMatchObject({
      title: "Finished Saga",
      earned: true,
    });
    // Earned first (newest first), then the locked ones.
    const firstLocked = m.findIndex((x) => !x.earned);
    expect(m.slice(0, firstLocked).every((x) => x.earned)).toBe(true);
    expect(m.slice(firstLocked).every((x) => !x.earned)).toBe(true);
    expect(m[0]!.id).toBe("series-s1");
  });

  it("lists years with reading, newest first", () => {
    expect(
      readingYears([d("2025-01-01"), d("2026-01-01"), d("2024-05-05", 0)]),
    ).toEqual([2026, 2025]);
  });
});
