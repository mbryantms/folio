import { describe, expect, it } from "vitest";

import {
  bucketIntensity,
  buildHeatmapGrid,
  dowHourBucket,
  formatDayLabel,
  formatDurationMs,
  formatTotalHours,
  groupSessionsByDay,
  labelForSession,
  movingAverage,
  sparklinePoints,
  timeOfDayBuckets,
} from "@/lib/activity";
import type { ReadingSessionView } from "@/lib/api/types";

function fakeSession(
  overrides: Partial<ReadingSessionView> & { started_at: string },
): ReadingSessionView {
  return {
    id: overrides.id ?? `session-${overrides.started_at}`,
    issue_id: overrides.issue_id ?? "issue-1",
    series_id: overrides.series_id ?? "series-1",
    client_session_id: overrides.client_session_id ?? "c-1",
    started_at: overrides.started_at,
    ended_at: overrides.ended_at ?? null,
    last_heartbeat_at: overrides.last_heartbeat_at ?? overrides.started_at,
    active_ms: overrides.active_ms ?? 60_000,
    distinct_pages_read: overrides.distinct_pages_read ?? 5,
    page_turns: overrides.page_turns ?? 6,
    start_page: overrides.start_page ?? 0,
    end_page: overrides.end_page ?? 4,
    furthest_page: overrides.furthest_page ?? 4,
    device: overrides.device ?? "web",
    view_mode: overrides.view_mode ?? "single",
  };
}

describe("groupSessionsByDay", () => {
  it("groups records by their UTC date prefix and preserves order", () => {
    const records = [
      fakeSession({ id: "a", started_at: "2026-05-06T22:30:00Z" }),
      fakeSession({ id: "b", started_at: "2026-05-06T09:00:00Z" }),
      fakeSession({ id: "c", started_at: "2026-05-05T14:00:00Z" }),
    ];
    const grouped = groupSessionsByDay(records);
    expect([...grouped.keys()]).toEqual(["2026-05-06", "2026-05-05"]);
    expect(grouped.get("2026-05-06")!.map((r) => r.id)).toEqual(["a", "b"]);
    expect(grouped.get("2026-05-05")!.map((r) => r.id)).toEqual(["c"]);
  });

  it("returns an empty map for empty input", () => {
    expect(groupSessionsByDay([])).toEqual(new Map());
  });
});

describe("formatDayLabel", () => {
  const today = new Date(2026, 4, 6); // local-time May 6

  it("emits Today / Yesterday / weekday for nearby days", () => {
    expect(formatDayLabel("2026-05-06", today)).toBe("Today");
    expect(formatDayLabel("2026-05-05", today)).toBe("Yesterday");
    // Anything older falls through to a locale-formatted string; assert it
    // contains the weekday name rather than pinning the exact format.
    const older = formatDayLabel("2026-05-01", today);
    expect(older).toMatch(/May|Friday/);
  });
});

describe("formatDurationMs", () => {
  it("uses seconds for under a minute", () => {
    expect(formatDurationMs(45_000)).toBe("45s");
  });
  it("uses minutes + seconds for under an hour", () => {
    expect(formatDurationMs(5 * 60_000)).toBe("5m 0s");
    expect(formatDurationMs(63_500)).toBe("1m 4s");
  });
  it("uses hours + minutes for an hour or more", () => {
    expect(formatDurationMs(90 * 60_000)).toBe("1h 30m");
    expect(formatDurationMs(60 * 60_000)).toBe("1h 0m");
  });
  it("clamps negatives to zero", () => {
    expect(formatDurationMs(-100)).toBe("0s");
  });
});

describe("formatTotalHours", () => {
  it("emits minutes when below an hour", () => {
    expect(formatTotalHours(0.5)).toBe("30m");
    expect(formatTotalHours(0.25)).toBe("15m");
  });
  it("emits a one-decimal hours figure when ≥ 1h", () => {
    expect(formatTotalHours(1.7)).toBe("1.7h");
    expect(formatTotalHours(10)).toBe("10.0h");
  });
});

describe("labelForSession", () => {
  const id = "d3076831c9a0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

  it("uses series + number + title when all present", () => {
    expect(
      labelForSession({
        issue_id: id,
        series_name: "Saga",
        issue_number: "1",
        issue_title: "The Will",
      }),
    ).toBe("Saga #1 · The Will");
  });
  it("falls back to series + number when title is missing", () => {
    expect(
      labelForSession({ issue_id: id, series_name: "Saga", issue_number: "1" }),
    ).toBe("Saga #1");
  });
  it("falls back to series + title when number is missing", () => {
    expect(
      labelForSession({
        issue_id: id,
        series_name: "Saga",
        issue_title: "Volume One",
      }),
    ).toBe("Saga — Volume One");
  });
  it("falls back to series alone", () => {
    expect(labelForSession({ issue_id: id, series_name: "Saga" })).toBe("Saga");
  });
  it("falls back to title alone if series is missing", () => {
    expect(labelForSession({ issue_id: id, issue_title: "One Shot" })).toBe(
      "One Shot",
    );
  });
  it("falls back to truncated hash when nothing is joined", () => {
    expect(labelForSession({ issue_id: id })).toBe("d3076831c9a0…");
  });
  it("ignores whitespace-only fields", () => {
    expect(
      labelForSession({
        issue_id: id,
        series_name: "Saga",
        issue_number: "  ",
        issue_title: "",
      }),
    ).toBe("Saga");
  });
});

describe("bucketIntensity", () => {
  it("returns 0 for empty values or zero max", () => {
    expect(bucketIntensity(0, 100)).toBe(0);
    expect(bucketIntensity(50, 0)).toBe(0);
    expect(bucketIntensity(-1, 100)).toBe(0);
  });
  it("buckets quartile-style", () => {
    expect(bucketIntensity(10, 100)).toBe(1); // 10% < 25%
    expect(bucketIntensity(40, 100)).toBe(2); // 25% <= 40% < 50%
    expect(bucketIntensity(60, 100)).toBe(3); // 50% <= 60% < 75%
    expect(bucketIntensity(80, 100)).toBe(4); // 75% <= 80%
    expect(bucketIntensity(100, 100)).toBe(4);
  });
});

describe("buildHeatmapGrid", () => {
  const today = new Date(2026, 4, 6); // Wed May 6 2026

  it("returns a 53-column × 7-row grid", () => {
    const grid = buildHeatmapGrid([], today);
    expect(grid.cells).toHaveLength(53);
    for (const col of grid.cells) {
      expect(col).toHaveLength(7);
    }
  });

  it("flags future cells as out-of-range", () => {
    const grid = buildHeatmapGrid([], today);
    // The week containing today is the rightmost column (col 52).
    // Cells beyond today (Thu, Fri, Sat = rows 4..6) should be out-of-range.
    const lastWeek = grid.cells[52]!;
    expect(lastWeek[3]!.inRange).toBe(true); // Wed = today
    expect(lastWeek[4]!.inRange).toBe(false); // Thu (future)
    expect(lastWeek[6]!.inRange).toBe(false); // Sat (future)
  });

  it("populates values from per-day rows and computes max", () => {
    const grid = buildHeatmapGrid(
      [
        { date: "2026-05-04", active_ms: 1_000 },
        { date: "2026-05-06", active_ms: 4_000 },
      ],
      today,
    );
    expect(grid.max).toBe(4_000);
    // May 4 (Mon) is in the rightmost column at row 1.
    const may4 = grid.cells[52]![1]!;
    expect(may4.date).toBe("2026-05-04");
    expect(may4.value).toBe(1_000);
    // 1000/4000 = 0.25 → falls into the 25..<50% bucket (intensity 2).
    expect(may4.intensity).toBe(2);
    const may6 = grid.cells[52]![3]!;
    expect(may6.date).toBe("2026-05-06");
    expect(may6.value).toBe(4_000);
    expect(may6.intensity).toBe(4);
  });

  it("emits month labels for the first column of each new month", () => {
    const grid = buildHeatmapGrid([], today);
    // We can't pin exact column positions across DST/locale, but there
    // should be ~12-13 month labels and they should be unique short names.
    expect(grid.monthLabels.length).toBeGreaterThanOrEqual(12);
    expect(grid.monthLabels.length).toBeLessThanOrEqual(13);
  });

  it("sums duplicate-date rows defensively", () => {
    const grid = buildHeatmapGrid(
      [
        { date: "2026-05-06", active_ms: 1_000 },
        { date: "2026-05-06", active_ms: 2_000 },
      ],
      today,
    );
    expect(grid.cells[52]![3]!.value).toBe(3_000);
  });
});

describe("dowHourBucket", () => {
  it("returns 0 for empty values or zero max", () => {
    expect(dowHourBucket(0, 100)).toBe(0);
    expect(dowHourBucket(50, 0)).toBe(0);
  });
  it("buckets peak-anchored", () => {
    // Any non-zero value below 25% lands in bucket 1 (not 0).
    expect(dowHourBucket(1, 100)).toBe(1);
    expect(dowHourBucket(25, 100)).toBe(2);
    expect(dowHourBucket(50, 100)).toBe(3);
    expect(dowHourBucket(75, 100)).toBe(4);
    expect(dowHourBucket(100, 100)).toBe(4);
  });
});

describe("movingAverage", () => {
  it("returns 0 for empty input", () => {
    expect(movingAverage([], 0, 5)).toBe(0);
  });
  it("averages the trailing window inclusive of end", () => {
    expect(movingAverage([10, 20, 30, 40, 50], 4, 3)).toBe((30 + 40 + 50) / 3);
  });
  it("clamps the window at the start of the series", () => {
    expect(movingAverage([10, 20, 30], 1, 5)).toBe((10 + 20) / 2);
  });
  it("clamps end past the array", () => {
    expect(movingAverage([10, 20], 99, 2)).toBe((10 + 20) / 2);
  });
});

describe("timeOfDayBuckets", () => {
  it("classifies hours into morning/afternoon/evening/night", () => {
    const got = timeOfDayBuckets([
      { hour: 6, active_ms: 1_000 }, // morning
      { hour: 11, active_ms: 2_000 }, // morning
      { hour: 13, active_ms: 4_000 }, // afternoon
      { hour: 18, active_ms: 8_000 }, // evening
      { hour: 23, active_ms: 16_000 }, // night
      { hour: 3, active_ms: 32_000 }, // night
    ]);
    expect(got.morning).toBe(3_000);
    expect(got.afternoon).toBe(4_000);
    expect(got.evening).toBe(8_000);
    expect(got.night).toBe(48_000);
  });
  it("returns zeros for empty input", () => {
    expect(timeOfDayBuckets([])).toEqual({
      morning: 0,
      afternoon: 0,
      evening: 0,
      night: 0,
    });
  });
});

describe("sparklinePoints", () => {
  it("returns an empty string for empty input", () => {
    expect(sparklinePoints([], 100, 20)).toBe("");
  });
  it("plots a single point at the right edge of the range", () => {
    const s = sparklinePoints([5], 100, 20);
    // Single data point: x = pad, y = baseline minus fully-extended bar.
    expect(s.split(" ")).toHaveLength(1);
  });
  it("normalizes against the series max so peaks hit the top", () => {
    const s = sparklinePoints([0, 5, 10], 100, 100);
    const points = s
      .split(" ")
      .map((p) => p.split(",").map(Number) as [number, number]);
    expect(points).toHaveLength(3);
    // First point is at the bottom (value 0).
    expect(points[0]![1]).toBeGreaterThan(points[2]![1]);
    // Last point is at/near the top (value = max).
    expect(points[2]![1]).toBeLessThanOrEqual(2);
  });
});
