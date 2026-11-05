import { describe, expect, it } from "vitest";

import { fillDayBuckets, lastUtcDays } from "@/lib/activity";

const b = (date: string, sessions = 1) => ({
  date,
  sessions,
  active_ms: sessions * 60_000,
  pages: sessions,
});

describe("fillDayBuckets", () => {
  it("fills quiet days between the first and last bucket", () => {
    const out = fillDayBuckets([b("2026-11-04", 2), b("2026-11-01")]);
    expect(out.map((d) => [d.date, d.sessions])).toEqual([
      ["2026-11-01", 1],
      ["2026-11-02", 0],
      ["2026-11-03", 0],
      ["2026-11-04", 2],
    ]);
  });

  it("covers a requested range, empty data included", () => {
    expect(
      fillDayBuckets([] as ReturnType<typeof b>[], {
        from: "2026-03-07",
        to: "2026-03-10",
      }).map((d) => d.date),
    ).toEqual(["2026-03-07", "2026-03-08", "2026-03-09", "2026-03-10"]);
  });

  it("returns empty input unchanged", () => {
    expect(fillDayBuckets([] as ReturnType<typeof b>[])).toEqual([]);
  });
});

describe("lastUtcDays", () => {
  it("ends today and spans the given days", () => {
    expect(lastUtcDays(14, new Date("2026-11-04T23:30:00Z"))).toEqual({
      from: "2026-10-22",
      to: "2026-11-04",
    });
  });
});
