import { describe, expect, it } from "vitest";

import { withoutWantedIssue } from "@/lib/api/mutations/upcoming";
import {
  indexMylarStatus,
  mylarAddMessage,
  mylarCanAdd,
  mylarEntryState,
} from "@/lib/cbl/mylar";

const index = indexMylarStatus({
  series: [
    { cv_id: 100, state: "in_mylar", error: null },
    { cv_id: 200, state: "requested", error: null },
    { cv_id: 300, state: "failed", error: "Mylar refused: Invalid ComicID" },
  ],
  issues: [
    { cv_id: 1001, state: "in_mylar", error: null },
    { cv_id: 1002, state: "requested", error: null },
    { cv_id: 1003, state: "failed", error: "never imported" },
  ],
});

describe("mylarEntryState", () => {
  it("cannot add an entry with no ComicVine series id", () => {
    const state = mylarEntryState(
      { cv_series_id: null, cv_issue_id: 5 },
      index,
    );
    expect(state).toEqual({ kind: "no_id" });
    expect(mylarCanAdd(state)).toBe(false);
  });

  it("is addable when Folio knows nothing about it", () => {
    const state = mylarEntryState({ cv_series_id: 999, cv_issue_id: 9 }, index);
    expect(state).toEqual({ kind: "addable" });
    expect(mylarCanAdd(state)).toBe(true);
  });

  it("lets the issue's state win over its series'", () => {
    expect(
      mylarEntryState({ cv_series_id: 200, cv_issue_id: 1001 }, index),
    ).toEqual({ kind: "wanted" });
    expect(
      mylarEntryState({ cv_series_id: 100, cv_issue_id: 1002 }, index),
    ).toEqual({ kind: "requested" });
    expect(
      mylarEntryState({ cv_series_id: 100, cv_issue_id: 1003 }, index),
    ).toEqual({ kind: "failed", error: "never imported" });
  });

  it("falls back to the series when the issue is unknown", () => {
    const inMylar = mylarEntryState(
      { cv_series_id: 100, cv_issue_id: 7 },
      index,
    );
    expect(inMylar).toEqual({ kind: "series_in_mylar" });
    expect(mylarCanAdd(inMylar)).toBe(true);
    expect(
      mylarEntryState({ cv_series_id: 200, cv_issue_id: null }, index),
    ).toEqual({ kind: "requested" });
    expect(
      mylarEntryState({ cv_series_id: 300, cv_issue_id: 8 }, index),
    ).toEqual({ kind: "failed", error: "Mylar refused: Invalid ComicID" });
  });
});

describe("mylarAddMessage", () => {
  const none = {
    series_requested: 0,
    issues_requested: 0,
    series_in_mylar: 0,
    already_requested: 0,
    without_comicvine_id: 0,
  };

  it("says what was queued", () => {
    expect(
      mylarAddMessage({ ...none, series_requested: 3, issues_requested: 1 }),
    ).toBe("Queued 3 series and 1 issue for Mylar");
  });

  it("names what was left out", () => {
    expect(
      mylarAddMessage({
        ...none,
        series_requested: 1,
        series_in_mylar: 2,
        already_requested: 4,
        without_comicvine_id: 1,
      }),
    ).toBe(
      "Queued 1 series for Mylar · 2 series already in Mylar · 4 already requested · 1 entry has no ComicVine id",
    );
  });

  it("says so when nothing was new", () => {
    expect(mylarAddMessage({ ...none, already_requested: 2 })).toBe(
      "Nothing new to send to Mylar · 2 already requested",
    );
  });
});

describe("withoutWantedIssue", () => {
  const check = {
    wanted: 10,
    owned_issues: 3,
    owned_series_total: 2,
    owned: [
      {
        cv_series_id: 100,
        series: "Alpha",
        issues: [
          { cv_issue_id: 1, issue_number: "1" },
          { cv_issue_id: 2, issue_number: "2" },
        ],
      },
      {
        cv_series_id: 200,
        series: "Beta",
        issues: [{ cv_issue_id: 3, issue_number: "1" }],
      },
    ],
    list_missing: 0,
    list_missing_not_wanted: 0,
  };

  it("drops the issue and keeps the series while it has others", () => {
    const next = withoutWantedIssue(check, 1);
    expect(next.wanted).toBe(9);
    expect(next.owned_issues).toBe(2);
    expect(next.owned_series_total).toBe(2);
    expect(next.owned[0]!.issues).toEqual([
      { cv_issue_id: 2, issue_number: "2" },
    ]);
  });

  it("drops a series with its last issue, and ignores an unknown id", () => {
    const next = withoutWantedIssue(check, 3);
    expect(next.owned.map((s) => s.cv_series_id)).toEqual([100]);
    expect(next.owned_series_total).toBe(1);
    expect(withoutWantedIssue(check, 99)).toEqual(check);
  });
});
