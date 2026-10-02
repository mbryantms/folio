/**
 * WP-7.7: the arc page's Tie-ins tab groups the role-ordered list into
 * contiguous sections, and the same-universe caption names what's shared.
 */
import { describe, expect, it } from "vitest";

import { groupTieIns } from "@/components/library/EntityDetail";
import { sharedCaption } from "@/components/library/SameUniverseSection";
import { arcTieInsNextPage } from "@/lib/api/queries";
import type { ArcTieInView } from "@/lib/api/types";

function tie(id: string, qualifier: ArcTieInView["qualifier"]): ArcTieInView {
  return {
    id,
    kind: "tie_in_to",
    kind_label: "Tie-in to",
    qualifier,
    source: "manual",
    created_at: "2026-01-01T00:00:00Z",
    series: { id, name: id } as ArcTieInView["series"],
  } as ArcTieInView;
}

describe("groupTieIns", () => {
  it("groups server-ordered roles into contiguous sections", () => {
    const groups = groupTieIns([
      tie("p", "prelude"),
      tie("m", "main"),
      tie("t1", null),
      tie("t2", "tie_in"),
      tie("a", "aftermath"),
    ]);
    expect(groups.map((g) => [g.label, g.items.map((i) => i.id)])).toEqual([
      ["Preludes", ["p"]],
      ["Main story", ["m"]],
      ["Tie-ins", ["t1", "t2"]],
      ["Aftermath", ["a"]],
    ]);
  });

  it("a later page extends the last group instead of reopening one", () => {
    const page1 = [tie("t1", null)];
    const page2 = [tie("t2", "tie_in"), tie("a", "aftermath")];
    expect(groupTieIns([...page1, ...page2]).map((g) => g.label)).toEqual([
      "Tie-ins",
      "Aftermath",
    ]);
  });

  it("walks next_cursor", () => {
    expect(arcTieInsNextPage({ next_cursor: "abc" })).toBe("abc");
    expect(arcTieInsNextPage({ next_cursor: null })).toBeUndefined();
  });
});

describe("sharedCaption", () => {
  it("names universes and series groups", () => {
    expect(
      sharedCaption([
        { via: "universe", name: "Mignolaverse" },
        { via: "series_group", name: "B.P.R.D." },
      ]),
    ).toBe("Universe: Mignolaverse · Group: B.P.R.D.");
  });
});
