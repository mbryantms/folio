import { describe, expect, it } from "vitest";

import {
  formatPageCount,
  formatPublicationDate,
  formatPublicationStatus,
  formatReadingTime,
  formatRelativeDate,
} from "@/lib/format";

describe("formatReadingTime", () => {
  it("returns null for missing or non-positive page counts", () => {
    expect(formatReadingTime(null)).toBeNull();
    expect(formatReadingTime(undefined)).toBeNull();
    expect(formatReadingTime(0)).toBeNull();
    expect(formatReadingTime(-3)).toBeNull();
  });

  it("formats short reads in minutes", () => {
    // 22 pages * 24s = 528s = 8.8 min → rounds to 9 min
    expect(formatReadingTime(22)).toBe("9 min");
  });

  it("formats long reads with hours and minutes", () => {
    // 200 pages * 24s = 4800s = 80 min = 1h 20m
    expect(formatReadingTime(200)).toBe("1 hr 20 min");
  });

  it("strips minutes when they round to zero", () => {
    // 150 pages * 24s = 3600s = 60 min = 1 hr exactly
    expect(formatReadingTime(150)).toBe("1 hr");
  });

  it("uses a custom seconds-per-page", () => {
    expect(formatReadingTime(60, 30)).toBe("30 min");
  });
});

describe("formatPageCount", () => {
  it("singularizes 1 page", () => {
    expect(formatPageCount(1)).toBe("1 page");
  });
  it("plural otherwise", () => {
    expect(formatPageCount(0)).toBe("0 pages");
    expect(formatPageCount(42)).toBe("42 pages");
  });
  it("returns null for missing", () => {
    expect(formatPageCount(null)).toBeNull();
    expect(formatPageCount(undefined)).toBeNull();
  });
});

describe("formatPublicationStatus", () => {
  it("title-cases snake_case inputs", () => {
    expect(formatPublicationStatus("on_hiatus")).toBe("On Hiatus");
  });
  it("returns null for missing", () => {
    expect(formatPublicationStatus(null)).toBeNull();
    expect(formatPublicationStatus(undefined)).toBeNull();
  });
});

describe("formatPublicationDate", () => {
  it("returns null without a year", () => {
    expect(formatPublicationDate(null)).toBeNull();
  });
  it("returns just the year when month/day missing", () => {
    expect(formatPublicationDate(2024)).toBe("2024");
  });
  it("renders month + year when day is missing", () => {
    // toLocaleDateString output is locale-dependent but contains the year.
    const out = formatPublicationDate(2024, 7) ?? "";
    expect(out).toMatch(/2024/);
    expect(out.length).toBeGreaterThan(4);
  });
  it("renders full date when day present", () => {
    const out = formatPublicationDate(2024, 7, 15) ?? "";
    expect(out).toMatch(/2024/);
    expect(out).toMatch(/15/);
  });
});

describe("formatRelativeDate", () => {
  const NOW = new Date("2026-05-05T12:00:00Z");

  it("returns null for missing input", () => {
    expect(formatRelativeDate(null, NOW)).toBeNull();
    expect(formatRelativeDate("not a date", NOW)).toBeNull();
  });

  it("uses 'just now' under a minute", () => {
    expect(formatRelativeDate("2026-05-05T11:59:30Z", NOW)).toBe("just now");
  });

  it("uses minutes under an hour", () => {
    expect(formatRelativeDate("2026-05-05T11:30:00Z", NOW)).toBe("30 min ago");
  });

  it("uses hours under a day", () => {
    expect(formatRelativeDate("2026-05-05T06:00:00Z", NOW)).toBe("6 hr ago");
  });

  it("uses 'yesterday' for ~1 day", () => {
    expect(formatRelativeDate("2026-05-04T12:00:00Z", NOW)).toBe("yesterday");
  });

  it("uses days for the rest of the week", () => {
    expect(formatRelativeDate("2026-05-02T12:00:00Z", NOW)).toBe("3 days ago");
  });
});
