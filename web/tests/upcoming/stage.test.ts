/**
 * Upcoming releases: the date and stage wording the page is built on.
 * Weeks are Monday–Sunday and turn over at the viewer's local midnight.
 */
import { describe, expect, it } from "vitest";

import type { UpcomingEntryView } from "@/lib/api/types";
import {
  coverMonth,
  localToday,
  mondayOf,
  releaseDayShort,
  sourceLabel,
  stageBadge,
  stageSentence,
  weekHeading,
  weekRange,
} from "@/lib/upcoming/stage";

function entry(over: Partial<UpcomingEntryView> = {}): UpcomingEntryView {
  return {
    key: "m1",
    source: "metron",
    series_name: "Action Comics",
    number: "1104",
    release_date: "2026-11-18",
    first_issue: false,
    owned: false,
    tracked: false,
    stage: "announced",
    ...over,
  };
}

describe("weeks", () => {
  it("start on Monday, including for a Sunday", () => {
    expect(mondayOf("2026-11-16")).toBe("2026-11-16");
    expect(mondayOf("2026-11-18")).toBe("2026-11-16");
    expect(mondayOf("2026-11-22")).toBe("2026-11-16");
    expect(mondayOf("2026-11-23")).toBe("2026-11-23");
  });

  it("are named relative to the viewer's date", () => {
    expect(weekHeading("2026-11-16", "2026-11-18")).toBe("This week");
    expect(weekHeading("2026-11-23", "2026-11-22")).toBe("Next week");
    expect(weekHeading("2026-11-30", "2026-11-18")).toBe("Week of Mon 30 Nov");
    expect(weekHeading("2027-01-04", "2026-12-30")).toBe("Next week");
    expect(weekHeading("2027-01-11", "2026-12-30")).toBe(
      "Week of Mon 11 Jan 2027",
    );
  });

  it("print their range, across a month end too", () => {
    expect(weekRange("2026-11-16", "2026-11-22")).toBe("16–22 Nov");
    expect(weekRange("2026-11-30", "2026-12-06")).toBe("30 Nov – 6 Dec");
  });
});

describe("dates", () => {
  it("uses the local calendar date, not UTC", () => {
    // 23:30 local on the 14th is still the 14th, whatever UTC says.
    expect(localToday(new Date(2026, 10, 14, 23, 30))).toBe("2026-11-14");
    expect(localToday(new Date(2026, 0, 5, 0, 5))).toBe("2026-01-05");
  });

  it("shortens the release day for the cover chip", () => {
    expect(releaseDayShort("2026-11-18")).toBe("Wed 18");
    expect(coverMonth("2027-01-01")).toBe("Jan 2027");
  });
});

describe("stage wording", () => {
  it("labels each stage", () => {
    expect(stageBadge(entry({ stage: "in_library" })).label).toBe("In library");
    expect(stageBadge(entry({ stage: "snatched" })).label).toBe("Downloading");
    expect(stageBadge(entry({ stage: "tracked" }))).toEqual({
      label: "Tracked",
      tone: "info",
    });
  });

  it("shows a failed download as failed, though Mylar still wants it", () => {
    const failed = entry({ stage: "wanted", mylar_status: "failed" });
    expect(stageBadge(failed)).toEqual({ label: "Failed", tone: "error" });
    expect(stageSentence(failed)).toMatch(/failed/);
    expect(
      stageBadge(entry({ stage: "wanted", mylar_status: "wanted" })),
    ).toEqual({ label: "Wanted", tone: "warning" });
  });

  it("names where the release was listed", () => {
    expect(sourceLabel(entry())).toBe("Metron's release calendar");
    expect(sourceLabel(entry({ source: "pull_list" }))).toBe(
      "Mylar's pull list",
    );
  });
});
