/**
 * Worklist cursor advance (B4 part 2 — auto-advance after apply).
 *
 * Guards the boundary the live flow can't cheaply re-assert on every run:
 * the *last* series' apply must finish the run, not index past the end.
 */
import { describe, expect, it } from "vitest";

import {
  nextWorklistIndex,
  worklistSummary,
} from "@/components/library/MetadataWorklistButton";

describe("nextWorklistIndex", () => {
  it("advances to the next item mid-queue", () => {
    expect(nextWorklistIndex(3, 0)).toEqual({ index: 1, done: false });
    expect(nextWorklistIndex(3, 1)).toEqual({ index: 2, done: false });
  });

  it("finishes when the last item is applied", () => {
    expect(nextWorklistIndex(3, 2)).toEqual({ index: 0, done: true });
  });

  it("a single-item queue finishes after one apply", () => {
    expect(nextWorklistIndex(1, 0)).toEqual({ index: 0, done: true });
  });
});

describe("worklistSummary (post-audit T1.6)", () => {
  it("reports confirmed applies and still-running ones separately", () => {
    expect(worklistSummary(3, 0)).toBe("Updated 3 series in the worklist");
    expect(worklistSummary(0, 1)).toBe(
      "1 apply is still running — see Background work",
    );
    expect(worklistSummary(2, 2)).toBe(
      "Updated 2 series in the worklist · 2 applies are still running — see Background work",
    );
    expect(worklistSummary(0, 0)).toBeNull();
  });
});
