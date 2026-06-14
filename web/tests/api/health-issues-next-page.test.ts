/**
 * Regression guard for the per-library health-issues pagination (frontend-audit
 * 2.7, D5 health half). The table moved from an unbounded fetch + client-side
 * status/severity/kind filtering to a server-paginated infinite query
 * (`useHealthIssuesInfinite`). This contracts the `getNextPageParam` callback so
 * a future refactor can't swallow `next_cursor` and silently truncate the table
 * — the exact failure class the list-pagination-completeness plan exists to
 * prevent.
 *
 * Mirrors web/tests/api/cbl-entries-next-page.test.ts.
 */
import { describe, expect, it } from "vitest";

import { healthIssuesNextPage } from "@/lib/api/queries";
import type { HealthIssuesPage } from "@/lib/api/types";

function page(overrides: Partial<HealthIssuesPage>): HealthIssuesPage {
  return {
    items: [],
    next_cursor: null,
    ...overrides,
  } as HealthIssuesPage;
}

describe("healthIssuesNextPage (useHealthIssuesInfinite cursor contract)", () => {
  it("returns the cursor string when next_cursor is present", () => {
    expect(healthIssuesNextPage(page({ next_cursor: "abc123==" }))).toBe(
      "abc123==",
    );
  });

  it("returns undefined when next_cursor is null", () => {
    expect(healthIssuesNextPage(page({ next_cursor: null }))).toBeUndefined();
  });

  it("returns undefined when next_cursor is missing", () => {
    // Server returns `Option<String>` — None serializes as null, but a future
    // schema change might omit the field entirely. Defensive.
    expect(
      healthIssuesNextPage(
        page({}) as HealthIssuesPage & { next_cursor?: never },
      ),
    ).toBeUndefined();
  });

  it("forwards an empty string verbatim (don't silently halt mid-walk)", () => {
    expect(healthIssuesNextPage(page({ next_cursor: "" }))).toBe("");
  });
});
