import { describe, expect, it } from "vitest";

import { applyBody } from "@/components/cbl/links/DeriveLinksDialog";
import type { DerivePreview, ProposedEventLink } from "@/lib/api/types";

function ev(
  path: string,
  after: number,
  order: number,
  extra: Partial<ProposedEventLink> = {},
): ProposedEventLink {
  return {
    catalog_path: path,
    name: path,
    total: 5,
    issues_here: 5,
    after_entry_id: `e${after}`,
    after_position: after,
    after_label: `X #${after}`,
    order_at_anchor: order,
    existing_list_id: null,
    already_linked: false,
    ...extra,
  };
}

const preview: DerivePreview = {
  twin_found: true,
  events: [
    ev("b.cbl", 7, 1),
    ev("a.cbl", 7, 0),
    ev("c.cbl", 3, 0),
    ev("d.cbl", 9, 0, { already_linked: true }),
  ],
  unplaced: [],
  parts: [
    { direction: "next", list_id: "p2", name: "Part 2", already_linked: false },
  ],
  imports_needed: 3,
};

describe("applyBody", () => {
  it("sends ticked, unlinked events in reading order", () => {
    const picked = new Set([
      "a.cbl@e7",
      "b.cbl@e7",
      "c.cbl@e3",
      "d.cbl@e9",
      "next:p2",
    ]);
    expect(applyBody(preview, picked)).toEqual({
      events: [
        { catalog_path: "c.cbl", after_entry_id: "e3" },
        { catalog_path: "a.cbl", after_entry_id: "e7" },
        { catalog_path: "b.cbl", after_entry_id: "e7" },
      ],
      parts: [{ direction: "next", list_id: "p2" }],
    });
  });

  it("leaves out what was unticked", () => {
    expect(applyBody(preview, new Set(["a.cbl@e7"]))).toEqual({
      events: [{ catalog_path: "a.cbl", after_entry_id: "e7" }],
      parts: [],
    });
  });
});
