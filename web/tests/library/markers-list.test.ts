/**
 * Markers + Collections M6 — pure-helper tests for the `/bookmarks`
 * index page. We don't render the component (vitest runs node-env
 * without a DOM and TanStack hooks need a QueryClient), but the grouping
 * and URL math live as exported helpers so we can pin them here.
 */
import { describe, expect, it } from "vitest";

import type { MarkerView } from "@/lib/api/types";
import {
  buildJumpHref,
  formatIssueLabel,
  groupBySeries,
} from "@/components/markers/MarkersList";

function marker(overrides: Partial<MarkerView> = {}): MarkerView {
  return {
    id: "00000000-0000-0000-0000-000000000001",
    user_id: "u1",
    series_id: "s1",
    issue_id: "i1",
    page_index: 0,
    kind: "bookmark",
    is_favorite: false,
    tags: [],
    region: null,
    selection: null,
    body: null,
    color: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    series_name: "Saga",
    series_slug: "saga",
    issue_slug: "saga-1",
    issue_title: "Chapter 1",
    issue_number: "1",
    ...overrides,
  };
}

describe("groupBySeries", () => {
  it("buckets markers under their series_id key", () => {
    const items = [
      marker({ id: "1", series_id: "s1", series_name: "Saga" }),
      marker({ id: "2", series_id: "s2", series_name: "Invincible" }),
      marker({ id: "3", series_id: "s1", series_name: "Saga" }),
    ];
    const groups = groupBySeries(items);
    expect(groups).toHaveLength(2);
    const saga = groups.find((g) => g.key === "s1");
    expect(saga?.items).toHaveLength(2);
    expect(saga?.label).toBe("Saga");
  });

  it("orders groups by most-recent updated_at then alpha", () => {
    const items = [
      marker({
        id: "old",
        series_id: "s1",
        series_name: "Alpha",
        updated_at: "2026-01-01T00:00:00Z",
      }),
      marker({
        id: "new",
        series_id: "s2",
        series_name: "Beta",
        updated_at: "2026-02-01T00:00:00Z",
      }),
    ];
    const groups = groupBySeries(items);
    expect(groups[0]!.key).toBe("s2");
    expect(groups[1]!.key).toBe("s1");
  });

  it("falls back to 'Unknown series' when series_name is missing", () => {
    const items = [marker({ series_name: null })];
    const groups = groupBySeries(items);
    expect(groups[0]!.label).toBe("Unknown series");
  });
});

describe("buildJumpHref", () => {
  it("produces a reader URL with ?page=<n>", () => {
    const href = buildJumpHref(
      marker({
        series_slug: "invincible",
        issue_slug: "invincible-7",
        page_index: 12,
      }),
    );
    expect(href).toBe("/read/invincible/invincible-7?page=12");
  });

  it("encodes slugs that contain spaces or special characters", () => {
    const href = buildJumpHref(
      marker({
        series_slug: "spider man",
        issue_slug: "issue/2",
        page_index: 0,
      }),
    );
    expect(href).toBe("/read/spider%20man/issue%2F2?page=0");
  });

  it("returns null when slug hydration is missing", () => {
    expect(buildJumpHref(marker({ series_slug: null }))).toBeNull();
    expect(buildJumpHref(marker({ issue_slug: null }))).toBeNull();
  });
});

describe("formatIssueLabel", () => {
  it("joins series name + #issue_number when both are present", () => {
    expect(formatIssueLabel(marker({ issue_number: "12" }))).toBe("Saga · #12");
  });

  it("falls back to issue_title when issue_number is null", () => {
    expect(
      formatIssueLabel(
        marker({
          issue_number: null,
          issue_title: "The Big Finale",
        }),
      ),
    ).toBe("Saga · The Big Finale");
  });

  it("returns 'Unknown issue' when both series_name and issue ids are missing", () => {
    expect(
      formatIssueLabel(
        marker({
          series_name: null,
          issue_number: null,
          issue_title: null,
        }),
      ),
    ).toBe("Unknown issue");
  });
});
