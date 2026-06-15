/**
 * Worklist cursor advance (B4 part 2 — auto-advance after apply).
 *
 * Guards the boundary the live flow can't cheaply re-assert on every run:
 * the *last* series' apply must finish the run, not index past the end.
 */
import { describe, expect, it } from "vitest";

import { nextWorklistIndex } from "@/components/library/MetadataWorklistButton";

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
