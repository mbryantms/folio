import { describe, expect, it } from "vitest";

import {
  biggestDay,
  busiestMonth,
  daysInYear,
  durationParts,
  hourLabel,
  inYear,
  monthBars,
  mosaicCovers,
  parseYear,
  pickerYears,
  readingRhythm,
  runLabel,
  storySlides,
  timeOfDay,
  topSeriesWithCovers,
  yearFetchWindow,
  yearLongestRun,
  type FinishedCover,
} from "@/lib/log/year";

const d = (date: string, pages = 10) => ({ date, pages });

describe("year bounds", () => {
  it("parses four-digit years only", () => {
    expect(parseYear("2026")).toBe(2026);
    expect(parseYear("26")).toBeNull();
    expect(parseYear("2026x")).toBeNull();
    expect(parseYear("0999")).toBeNull();
  });

  it("pads the fetch window a day each side and filters by local date", () => {
    expect(yearFetchWindow(2026)).toEqual({
      from: "2025-12-31T00:00:00Z",
      to: "2027-01-02T00:00:00Z",
    });
    // 23:30 UTC on 31 Dec is already 1 Jan in Luxembourg.
    expect(inYear("2025-12-31T23:30:00Z", 2026, "Europe/Luxembourg")).toBe(
      true,
    );
    expect(inYear("2025-12-31T23:30:00Z", 2026, "UTC")).toBe(false);
  });

  it("knows leap years", () => {
    expect(daysInYear(2024)).toBe(366);
    expect(daysInYear(2026)).toBe(365);
    expect(daysInYear(2000)).toBe(366);
    expect(daysInYear(2100)).toBe(365);
  });
});

describe("months", () => {
  const days = [
    d("2026-01-05", 30),
    d("2026-03-01", 50),
    d("2026-03-02", 25),
    d("2026-10-10", 75),
    d("2025-12-31", 999),
  ];

  it("builds twelve bars for the year only", () => {
    const bars = monthBars(days, 2026);
    expect(bars).toHaveLength(12);
    expect(bars[0]).toEqual({ key: "2026-01", label: "Jan", pages: 30 });
    expect(bars[2]!.pages).toBe(75);
    expect(bars[11]!.pages).toBe(0);
  });

  it("picks the busiest month, earliest on a tie, -1 when empty", () => {
    expect(busiestMonth(monthBars(days, 2026))).toBe(2);
    expect(busiestMonth(monthBars([], 2026))).toBe(-1);
  });

  it("finds the biggest day", () => {
    expect(biggestDay(days.slice(0, 4))).toEqual(d("2026-10-10", 75));
    expect(biggestDay([d("2026-01-02", 5), d("2026-01-01", 5)])).toEqual(
      d("2026-01-01", 5),
    );
    expect(biggestDay([])).toBeNull();
  });
});

describe("yearLongestRun", () => {
  it("keeps runs inside the year and labels them", () => {
    const days = [
      d("2025-12-30"),
      d("2025-12-31"),
      d("2026-01-01"),
      d("2026-01-02"),
      d("2026-05-01"),
      d("2026-05-02"),
      d("2026-05-03"),
    ];
    const run = yearLongestRun(days, 2026)!;
    expect(run).toEqual({ start: "2026-05-01", end: "2026-05-03", length: 3 });
    expect(runLabel(run)).toBe("Fri 1 May – Sun 3 May");
    expect(
      runLabel({ start: "2026-05-01", end: "2026-05-01", length: 1 }),
    ).toBe("Fri 1 May");
    expect(yearLongestRun([], 2026)).toBeNull();
  });
});

describe("readingRhythm", () => {
  it("maps Sunday-first cells to Monday-first weekdays and time of day", () => {
    const r = readingRhythm([
      { dow: 0, hour: 21, active_ms: 3_000, sessions: 1 }, // Sun evening
      { dow: 0, hour: 23, active_ms: 5_000, sessions: 1 }, // Sun night
      { dow: 1, hour: 9, active_ms: 1_000, sessions: 4 }, // Mon morning
    ])!;
    expect(r.byDow[6]).toBe(8_000);
    expect(r.byDow[0]).toBe(1_000);
    expect(r.peakDow).toBe(6);
    expect(r.peakHour).toBe(23);
    expect(r.peakTime).toBe("night");
    expect(r.persona).toBe("a night owl");
    const shares = Object.fromEntries(r.byTime.map((t) => [t.id, t.share]));
    expect(shares.night).toBeCloseTo(5 / 9);
    expect(shares.afternoon).toBe(0);
  });

  it("falls back to sessions, and is null with no data", () => {
    const r = readingRhythm([{ dow: 3, hour: 13, active_ms: 0, sessions: 2 }])!;
    expect(r.peakTime).toBe("afternoon");
    expect(readingRhythm([])).toBeNull();
  });

  it("buckets hours like the server", () => {
    expect([4, 5, 11, 12, 16, 17, 21, 22].map(timeOfDay)).toEqual([
      "night",
      "morning",
      "morning",
      "afternoon",
      "afternoon",
      "evening",
      "evening",
      "night",
    ]);
    expect([0, 9, 12, 21].map(hourLabel)).toEqual([
      "midnight",
      "9 am",
      "noon",
      "9 pm",
    ]);
  });
});

describe("durationParts", () => {
  it("uses hours from one hour up, else minutes", () => {
    expect(durationParts(5 * 3_600_000)).toEqual({ value: 5, unit: "hours" });
    expect(durationParts(3_600_000)).toEqual({ value: 1, unit: "hour" });
    expect(durationParts(35 * 60_000)).toEqual({ value: 35, unit: "minutes" });
  });
});

const fin = (
  id: string,
  seriesId: string | null,
  cover: string | null,
  seriesCover: string | null = null,
  issueId: string | null = id,
): FinishedCover => ({
  id: `f:${id}`,
  issueId,
  seriesId,
  seriesName: seriesId ?? "x",
  cover,
  seriesCover,
});

describe("covers", () => {
  it("dedupes the mosaic by issue and caps it, counting every issue", () => {
    const m = mosaicCovers(
      [
        fin("a", "s1", "a.jpg"),
        fin("a", "s1", "a.jpg"),
        fin("b", "s1", null, "s1.jpg"),
        fin("c", "s2", null),
        fin("d", "s2", "d.jpg"),
      ],
      2,
    );
    expect(m.total).toBe(4);
    expect(m.covers.map((c) => c.src)).toEqual(["a.jpg", "s1.jpg"]);
  });

  it("prefers a series cover for top series, then an issue cover", () => {
    const top = topSeriesWithCovers(
      [
        { series_id: "s1", name: "One", active_ms: 9, sessions: 3 },
        { series_id: "s2", name: "Two", active_ms: 5, sessions: 1 },
        { series_id: "s3", name: "Three", active_ms: 1, sessions: 1 },
      ],
      [
        fin("a", "s1", "a.jpg"),
        fin("b", "s1", "b.jpg", "s1.jpg"),
        fin("c", "s2", "c.jpg"),
      ],
    );
    expect(top.map((t) => [t.cover, t.finished])).toEqual([
      ["s1.jpg", 2],
      ["c.jpg", 1],
      [null, 0],
    ]);
  });
});

describe("pickerYears", () => {
  it("lists years with reading plus the one shown, newest first", () => {
    expect(
      pickerYears([d("2024-03-01"), d("2026-01-01"), d("2025-02-02", 0)], 2023),
    ).toEqual([2026, 2024, 2023]);
  });
});

describe("storySlides", () => {
  const base = {
    pages: 100,
    sessions: 5,
    daysActive: 3,
    busiest: 2,
    topCount: 2,
    hasRhythm: true,
    run: { start: "2026-01-01", end: "2026-01-03", length: 3 },
    finished: 4,
  };

  it("runs the full story when everything has data", () => {
    expect(storySlides(base)).toEqual([
      "intro",
      "numbers",
      "months",
      "top",
      "rhythm",
      "streak",
      "mosaic",
      "outro",
    ]);
  });

  it("skips slides without data and collapses an empty year", () => {
    expect(
      storySlides({
        ...base,
        topCount: 0,
        hasRhythm: false,
        run: { start: "2026-01-01", end: "2026-01-01", length: 1 },
        finished: 0,
      }),
    ).toEqual(["intro", "numbers", "months", "outro"]);
    expect(
      storySlides({ ...base, pages: 0, sessions: 0, daysActive: 0 }),
    ).toEqual(["empty"]);
  });
});
