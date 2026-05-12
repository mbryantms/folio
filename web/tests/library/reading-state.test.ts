import { describe, expect, it } from "vitest";

import {
  indexProgress,
  pickNextIssue,
  readButtonLabel,
  readStateFor,
  type ProgressLike,
} from "@/lib/reading-state";
import type { IssueSummaryView } from "@/lib/api/types";

const issue = (overrides: Partial<IssueSummaryView>): IssueSummaryView => ({
  id: "i1",
  slug: "1",
  series_id: "s1",
  series_slug: "test-series",
  title: null,
  number: "1",
  sort_number: 1,
  year: null,
  page_count: 22,
  state: "active",
  cover_url: null,
  created_at: "2025-01-01T00:00:00Z",
  updated_at: "2025-01-01T00:00:00Z",
  ...overrides,
});

const progress = (overrides: Partial<ProgressLike>): ProgressLike => ({
  issue_id: "i1",
  page: 0,
  finished: false,
  updated_at: "2025-01-01T00:00:00Z",
  ...overrides,
});

describe("readStateFor", () => {
  it("returns unread when there's no progress record", () => {
    expect(readStateFor(issue({}), null)).toBe("unread");
    expect(readStateFor(issue({}), undefined)).toBe("unread");
  });

  it("returns unread for a progress row at page 0 not finished", () => {
    expect(readStateFor(issue({}), progress({ page: 0 }))).toBe("unread");
  });

  it("returns in_progress when page > 0 and not finished", () => {
    expect(readStateFor(issue({}), progress({ page: 5 }))).toBe("in_progress");
  });

  it("returns finished when finished is true regardless of page", () => {
    expect(readStateFor(issue({}), progress({ finished: true, page: 0 }))).toBe(
      "finished",
    );
    expect(
      readStateFor(issue({}), progress({ finished: true, page: 21 })),
    ).toBe("finished");
  });
});

describe("readButtonLabel", () => {
  it("maps states to user-facing copy", () => {
    expect(readButtonLabel("unread")).toBe("Read");
    expect(readButtonLabel("in_progress")).toBe("Continue reading");
    expect(readButtonLabel("finished")).toBe("Read again");
  });
});

describe("pickNextIssue", () => {
  it("returns null target for an empty active list", () => {
    const out = pickNextIssue([], new Map());
    expect(out.target).toBeNull();
    expect(out.state).toBe("unread");
  });

  it("ignores soft-deleted issues", () => {
    const removed = issue({ id: "i-removed", state: "removed" });
    const active = issue({ id: "i-active" });
    const out = pickNextIssue([removed, active], new Map());
    expect(out.target?.id).toBe("i-active");
    expect(out.state).toBe("unread");
  });

  it("prefers the most-recently-updated in-progress issue", () => {
    const issues = [
      issue({ id: "i1", number: "1" }),
      issue({ id: "i2", number: "2" }),
      issue({ id: "i3", number: "3" }),
    ];
    const map = indexProgress([
      progress({ issue_id: "i1", page: 5, updated_at: "2025-02-01T00:00:00Z" }),
      progress({ issue_id: "i3", page: 8, updated_at: "2025-03-01T00:00:00Z" }),
    ]);
    const out = pickNextIssue(issues, map);
    expect(out.target?.id).toBe("i3");
    expect(out.state).toBe("in_progress");
  });

  it("falls through to the first not-finished issue when nothing is in-progress", () => {
    const issues = [
      issue({ id: "i1", number: "1" }),
      issue({ id: "i2", number: "2" }),
      issue({ id: "i3", number: "3" }),
    ];
    const map = indexProgress([
      progress({ issue_id: "i1", finished: true, page: 21 }),
    ]);
    const out = pickNextIssue(issues, map);
    expect(out.target?.id).toBe("i2");
    expect(out.state).toBe("unread");
  });

  it("returns Read again on the first issue when every issue is finished", () => {
    const issues = [
      issue({ id: "i1", number: "1" }),
      issue({ id: "i2", number: "2" }),
    ];
    const map = indexProgress([
      progress({ issue_id: "i1", finished: true, page: 21 }),
      progress({ issue_id: "i2", finished: true, page: 21 }),
    ]);
    const out = pickNextIssue(issues, map);
    expect(out.target?.id).toBe("i1");
    expect(out.state).toBe("finished");
  });

  it("treats a record with page=0 not-finished as still unread", () => {
    // E.g. the user opened the reader, the writer fired with page=0, but
    // they navigated away. Should not classify as in_progress.
    const issues = [issue({ id: "i1" }), issue({ id: "i2" })];
    const map = indexProgress([
      progress({ issue_id: "i1", page: 0, finished: false }),
    ]);
    const out = pickNextIssue(issues, map);
    expect(out.target?.id).toBe("i1");
    expect(out.state).toBe("unread");
  });
});

describe("indexProgress", () => {
  it("filters by issue id set when provided", () => {
    const records: ProgressLike[] = [
      progress({ issue_id: "a" }),
      progress({ issue_id: "b" }),
      progress({ issue_id: "c" }),
    ];
    const map = indexProgress(records, new Set(["a", "c"]));
    expect(map.has("a")).toBe(true);
    expect(map.has("b")).toBe(false);
    expect(map.has("c")).toBe(true);
  });
});
