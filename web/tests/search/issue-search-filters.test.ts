/** URL ↔ state plumbing for the `/search?category=issues` read-status
 *  facet. Round-trip + edge-case coverage for the parse/serialize helpers
 *  behind the issue filter sheet. */
import { describe, expect, it } from "vitest";

import {
  ISSUE_READ_STATUS_OPTIONS,
  issueReadStatusToParam,
  parseIssueReadStatus,
} from "@/lib/search/issue-search-filters";

describe("parseIssueReadStatus", () => {
  it("returns [] for empty input", () => {
    expect(parseIssueReadStatus({})).toEqual([]);
    expect(parseIssueReadStatus({ read_status: "" })).toEqual([]);
  });

  it("parses a valid CSV, preserving order", () => {
    expect(parseIssueReadStatus({ read_status: "read,unread" })).toEqual([
      "read",
      "unread",
    ]);
  });

  it("drops unknown tokens and de-dupes", () => {
    expect(
      parseIssueReadStatus({ read_status: "unread,bogus,unread,read" }),
    ).toEqual(["unread", "read"]);
  });

  it("trims whitespace around tokens", () => {
    expect(
      parseIssueReadStatus({ read_status: " in_progress , read " }),
    ).toEqual(["in_progress", "read"]);
  });
});

describe("issueReadStatusToParam", () => {
  it("returns undefined for empty / all-three (both no-ops)", () => {
    expect(issueReadStatusToParam([])).toBeUndefined();
    expect(
      issueReadStatusToParam(["unread", "in_progress", "read"]),
    ).toBeUndefined();
  });

  it("serializes a partial selection to CSV", () => {
    expect(issueReadStatusToParam(["unread", "in_progress"])).toBe(
      "unread,in_progress",
    );
  });

  it("ignores invalid tokens before deciding no-op", () => {
    expect(issueReadStatusToParam(["unread", "bogus"])).toBe("unread");
    expect(issueReadStatusToParam(["bogus"])).toBeUndefined();
  });
});

describe("round-trip", () => {
  it("parse ∘ toParam is identity for a partial selection", () => {
    const param = issueReadStatusToParam(["read"]);
    expect(param).toBeDefined();
    expect(parseIssueReadStatus({ read_status: param })).toEqual(["read"]);
  });
});

describe("ISSUE_READ_STATUS_OPTIONS", () => {
  it("matches the server's three-state vocabulary", () => {
    expect(ISSUE_READ_STATUS_OPTIONS.map((o) => o.value)).toEqual([
      "unread",
      "in_progress",
      "read",
    ]);
  });
});
