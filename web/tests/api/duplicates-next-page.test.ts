/**
 * Regression guard for the Duplicates page pagination (WP-3.3). The list is
 * a cursor-paginated infinite query (`useDuplicatesInfinite`); this pins the
 * `getNextPageParam` contract so a refactor can't swallow `next_cursor` and
 * silently truncate the group list.
 *
 * Mirrors web/tests/api/removed-items-next-page.test.ts.
 */
import { describe, expect, it } from "vitest";

import { duplicatesNextPage } from "@/lib/api/queries";
import type { DuplicateListView } from "@/lib/api/types";

function page(overrides: Partial<DuplicateListView>): DuplicateListView {
  return { items: [], next_cursor: null, ...overrides } as DuplicateListView;
}

describe("duplicatesNextPage (useDuplicatesInfinite cursor contract)", () => {
  it("returns the cursor string when next_cursor is present", () => {
    expect(duplicatesNextPage(page({ next_cursor: "abc123" }))).toBe("abc123");
  });

  it("returns undefined when next_cursor is null", () => {
    expect(duplicatesNextPage(page({ next_cursor: null }))).toBeUndefined();
  });

  it("forwards an empty string verbatim (don't silently halt mid-walk)", () => {
    expect(duplicatesNextPage(page({ next_cursor: "" }))).toBe("");
  });
});
