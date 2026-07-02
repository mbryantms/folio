/**
 * Regression guard for the removed-items pagination (audit UX-11). The table
 * moved from an unbounded one-shot fetch to a server-paginated infinite query
 * (`useRemovedItemsInfinite`). This contracts the `getNextPageParam` callback
 * so a future refactor can't swallow `next_cursor` and silently truncate the
 * table — the exact failure class the list-pagination-completeness plan
 * exists to prevent.
 *
 * Mirrors web/tests/api/health-issues-next-page.test.ts.
 */
import { describe, expect, it } from "vitest";

import { removedItemsNextPage } from "@/lib/api/queries";
import type { RemovedListView } from "@/lib/api/types";

function page(overrides: Partial<RemovedListView>): RemovedListView {
  return {
    issues: [],
    series: [],
    next_cursor: null,
    ...overrides,
  } as RemovedListView;
}

describe("removedItemsNextPage (useRemovedItemsInfinite cursor contract)", () => {
  it("returns the cursor string when next_cursor is present", () => {
    expect(removedItemsNextPage(page({ next_cursor: "abc123==" }))).toBe(
      "abc123==",
    );
  });

  it("returns undefined when next_cursor is null", () => {
    expect(removedItemsNextPage(page({ next_cursor: null }))).toBeUndefined();
  });

  it("returns undefined when next_cursor is missing", () => {
    expect(
      removedItemsNextPage(
        page({}) as RemovedListView & { next_cursor?: never },
      ),
    ).toBeUndefined();
  });

  it("forwards an empty string verbatim (don't silently halt mid-walk)", () => {
    expect(removedItemsNextPage(page({ next_cursor: "" }))).toBe("");
  });
});
