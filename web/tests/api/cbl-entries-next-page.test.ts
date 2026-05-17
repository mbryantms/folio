/**
 * Regression guard for the list-pagination-completeness 1.0 plan
 * (shipped 2026-05-14, commit 7c9d0b6).
 *
 * Original bug: `GET /me/cbl-lists/{id}` defaulted `limit.unwrap_or(500)`
 * and the UI fetched it as a single page, silently dropping every
 * entry past position 500. The fix moved entries to a paginated
 * endpoint walked by `useCblListEntriesInfinite`; this test contracts
 * the `getNextPageParam` callback so a future refactor can't swallow
 * `next_cursor` and re-introduce a truncation by another name.
 *
 * What the callback MUST do:
 *   - Return `last.next_cursor` (string) when the server signals
 *     "another page available."
 *   - Return `undefined` when `next_cursor` is `null` so TanStack's
 *     `useInfiniteQuery` halts the walk.
 *   - Return `undefined` when `next_cursor` is missing entirely
 *     (defensive: the field is `Option<String>` server-side, and
 *     omitted means "no more pages").
 *
 * It MUST NOT:
 *   - Coerce a non-string cursor to `String()`.
 *   - Treat an empty string as "no more pages" — the server should
 *     never emit `""`, but if it does we want the loop to make
 *     forward progress (or fail loudly server-side), not silently
 *     halt mid-walk.
 */
import { describe, expect, it } from "vitest";

import { cblEntriesNextPage } from "@/lib/api/queries";
import type { CblEntryListView } from "@/lib/api/types";

function page(overrides: Partial<CblEntryListView>): CblEntryListView {
  return {
    items: [],
    next_cursor: null,
    total: null,
    ...overrides,
  } as CblEntryListView;
}

describe("cblEntriesNextPage (useCblListEntriesInfinite cursor contract)", () => {
  it("returns the cursor string when next_cursor is present", () => {
    const result = cblEntriesNextPage(page({ next_cursor: "abc123==" }));
    expect(result).toBe("abc123==");
  });

  it("returns undefined when next_cursor is null", () => {
    const result = cblEntriesNextPage(page({ next_cursor: null }));
    expect(result).toBeUndefined();
  });

  it("returns undefined when next_cursor is missing", () => {
    // Server returns `Option<String>` — None serializes as null, but a
    // future schema change might omit the field entirely. Defensive.
    const result = cblEntriesNextPage(
      page({}) as CblEntryListView & { next_cursor?: never },
    );
    expect(result).toBeUndefined();
  });

  it("forwards an empty string verbatim (don't silently halt mid-walk)", () => {
    // The server never emits "", but if it did we want TanStack to
    // make a request with an empty cursor and surface whatever
    // happens — silently halting would re-introduce the original
    // truncation bug class.
    const result = cblEntriesNextPage(page({ next_cursor: "" }));
    expect(result).toBe("");
  });
});
