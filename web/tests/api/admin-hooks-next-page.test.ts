/**
 * Cursor contract for `cursorNextPage`, the shared `getNextPageParam` of the
 * three admin hooks the 2026-11 audit (N-O7) found still on `useQuery` over
 * a `next_cursor` response: `useScanRuns`, `useScanBatches`,
 * `useAdminHealthIssues`. Mirrors web/tests/api/cbl-entries-next-page.test.ts:
 * a string cursor walks on, `null` / missing halts, and an empty string is
 * forwarded verbatim rather than silently halting mid-walk. The hooks'
 * wiring to this helper is covered by admin-hooks-infinite.test.tsx.
 */
import { describe, expect, it } from "vitest";

import { cursorNextPage } from "@/lib/api/queries";

describe("cursorNextPage (useScanRuns / useScanBatches / useAdminHealthIssues)", () => {
  it("returns the cursor string when next_cursor is present", () => {
    expect(cursorNextPage({ next_cursor: "abc123==" })).toBe("abc123==");
  });

  it("returns undefined when next_cursor is null (stop the walk)", () => {
    expect(cursorNextPage({ next_cursor: null })).toBeUndefined();
  });

  it("returns undefined when next_cursor is missing", () => {
    expect(cursorNextPage({})).toBeUndefined();
  });

  it("forwards an empty string verbatim (don't silently halt mid-walk)", () => {
    expect(cursorNextPage({ next_cursor: "" })).toBe("");
  });
});
