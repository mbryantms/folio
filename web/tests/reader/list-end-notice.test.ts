/**
 * `listEndTarget`: the end card shows "back to the list" only when Up
 * Next ended with a list to return to and no issue to open.
 */
import { describe, expect, it } from "vitest";

import { listEndTarget } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ListEndNotice";
import type { NextUpView } from "@/lib/api/types";

const sw = {
  reason: "return" as const,
  list_id: "list-1",
  collection: false,
  name: "[Marvel] Master Part #1 (WEB-CBRO)",
  view_id: "view-1",
};

describe("listEndTarget", () => {
  it("is the list when there is no issue to open", () => {
    expect(listEndTarget({ source: "none", switch_to: sw } as NextUpView)).toBe(
      sw,
    );
  });
  it("is null when Up Next has a target (a normal switch)", () => {
    const data = {
      source: "cbl",
      switch_to: sw,
      target: { id: "i" },
    } as unknown as NextUpView;
    expect(listEndTarget(data)).toBeNull();
  });
  it("is null with no switch, a collection switch, or no data", () => {
    expect(listEndTarget({ source: "none" } as NextUpView)).toBeNull();
    expect(
      listEndTarget({
        source: "none",
        switch_to: { ...sw, collection: true, list_id: null },
      } as NextUpView),
    ).toBeNull();
    expect(listEndTarget(undefined)).toBeNull();
  });
});
