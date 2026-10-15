/**
 * `nextUpListSubtitle` — the shared "Issue N of M in {list}" line under
 * the next-up target when the resolver followed a CBL or a collection.
 */
import { describe, expect, it } from "vitest";

import type { NextUpView } from "@/lib/api/types";
import { nextUpListSubtitle } from "@/lib/reader/list-context";

describe("nextUpListSubtitle", () => {
  it("is null without data or for a series / none result", () => {
    expect(nextUpListSubtitle(undefined)).toBeNull();
    expect(nextUpListSubtitle({ source: "series" } as NextUpView)).toBeNull();
    expect(nextUpListSubtitle({ source: "none" } as NextUpView)).toBeNull();
  });

  it("words a CBL and a collection result identically", () => {
    expect(
      nextUpListSubtitle({
        source: "cbl",
        cbl_position: 4,
        cbl_total: 24,
        cbl_list_name: "Best of 2003",
      } as NextUpView),
    ).toBe("Issue 4 of 24 in Best of 2003");
    expect(
      nextUpListSubtitle({
        source: "collection",
        collection_position: 2,
        collection_total: 9,
        collection_name: "Summer pile",
      } as NextUpView),
    ).toBe("Issue 2 of 9 in Summer pile");
  });

  it("ignores list fields that do not match the source", () => {
    expect(
      nextUpListSubtitle({
        source: "collection",
        cbl_position: 4,
        cbl_total: 24,
        cbl_list_name: "Best of 2003",
      } as NextUpView),
    ).toBeNull();
  });
});
