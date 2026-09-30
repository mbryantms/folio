/**
 * `<HashBackfillProgress>` (WP-3.2 first-import lazy-hash mode).
 *
 * Static-markup render with the query/mutation hooks mocked: the panel is
 * hidden when nothing is pending and shows hashed/total + percent while
 * the background drain runs.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

const state: {
  data:
    | {
        pending: number;
        total: number;
        hashed: number;
        enabled: boolean;
        first_import_active: boolean;
      }
    | undefined;
} = { data: undefined };

vi.mock("@/lib/api/queries", () => ({
  useHashBackfill: () => ({ data: state.data, isLoading: false }),
}));

vi.mock("@/lib/api/mutations", () => ({
  useStartHashBackfill: () => ({ mutate: () => undefined, isPending: false }),
}));

import {
  HashBackfillProgress,
  hashBackfillPercent,
} from "@/components/admin/library/HashBackfillProgress";

function render() {
  return renderToStaticMarkup(
    createElement(HashBackfillProgress, { librarySlug: "main" }),
  );
}

describe("hashBackfillPercent", () => {
  it("rounds hashed/total and treats an empty library as done", () => {
    expect(hashBackfillPercent({ hashed: 1, total: 3 })).toBe(33);
    expect(hashBackfillPercent({ hashed: 3, total: 3 })).toBe(100);
    expect(hashBackfillPercent({ hashed: 0, total: 0 })).toBe(100);
  });
});

describe("<HashBackfillProgress>", () => {
  it("renders nothing when no hash is pending", () => {
    state.data = {
      pending: 0,
      total: 10,
      hashed: 10,
      enabled: true,
      first_import_active: false,
    };
    expect(render()).toBe("");
    state.data = undefined;
    expect(render()).toBe("");
  });

  it("shows progress while the drain runs", () => {
    state.data = {
      pending: 750,
      total: 1000,
      hashed: 250,
      enabled: true,
      first_import_active: false,
    };
    const html = render();
    expect(html).toContain("Content hashing in progress");
    expect(html).toContain("(25%)");
    expect(html).toContain("still");
    expect(html).toContain("Resume");
  });
});
