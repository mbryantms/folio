/**
 * Pure-helper unit tests for the bulk-archive-edit skip summary (audit B17).
 */
import { describe, expect, it } from "vitest";

import {
  skippedEntryIds,
  skippedIssueIds,
  summarizeBulkArchiveSkips,
} from "@/lib/library/bulk-archive-skips";

describe("summarizeBulkArchiveSkips", () => {
  it("groups by reason and counts", () => {
    const summary = summarizeBulkArchiveSkips([
      { issue_id: "a", reason: "archive writeback disabled for this library" },
      { issue_id: "b", reason: "archive writeback disabled for this library" },
      {
        issue_id: "c",
        reason: "unsupported archive format (CBZ/CBT/CBR only)",
      },
    ]);
    expect(summary).toBe(
      "2 archive writeback disabled for this library; 1 unsupported archive format (CBZ/CBT/CBR only)",
    );
  });

  it("orders the highest-count reason first", () => {
    const summary = summarizeBulkArchiveSkips([
      { issue_id: "a", reason: "issue not found" },
      { issue_id: "b", reason: "could not enqueue" },
      { issue_id: "c", reason: "could not enqueue" },
    ]);
    expect(summary).toBe("2 could not enqueue; 1 issue not found");
  });

  it("renders a single reason without a separator", () => {
    expect(
      summarizeBulkArchiveSkips([{ issue_id: "a", reason: "issue not found" }]),
    ).toBe("1 issue not found");
  });

  it("is empty for no skips", () => {
    expect(summarizeBulkArchiveSkips([])).toBe("");
  });
});

describe("skippedIssueIds", () => {
  it("extracts the issue ids in order", () => {
    expect(
      skippedIssueIds([
        { issue_id: "x", reason: "issue not found" },
        { issue_id: "y", reason: "could not enqueue" },
      ]),
    ).toEqual(["x", "y"]);
  });
});

describe("skippedEntryIds", () => {
  const entries = [
    { entryId: "e1", issueId: "i1" },
    { entryId: "e2", issueId: "i2" },
    { entryId: "e3", issueId: null }, // series / placeholder
    { entryId: "e4", issueId: "i4" },
  ];

  it("maps skipped issue ids back to their entry ids in entries order", () => {
    expect(
      skippedEntryIds(
        [
          { issue_id: "i4", reason: "x" },
          { issue_id: "i1", reason: "x" },
        ],
        entries,
      ),
    ).toEqual(["e1", "e4"]);
  });

  it("ignores entries with no resolved issue", () => {
    expect(skippedEntryIds([{ issue_id: "i2", reason: "x" }], entries)).toEqual(
      ["e2"],
    );
  });

  it("is empty when nothing matches", () => {
    expect(
      skippedEntryIds([{ issue_id: "nope", reason: "x" }], entries),
    ).toEqual([]);
  });
});
