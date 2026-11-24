import { describe, expect, it } from "vitest";

import { orderParents, parentHref } from "@/components/cbl/links/PartOfLink";
import type { NextUpView, ReadingListParentView } from "@/lib/api/types";
import { nextUpListSubtitle } from "@/lib/reader/list-context";

function parent(
  name: string,
  view: string | null,
  at: number | null = 3,
): ReadingListParentView {
  return {
    link_id: `l-${name}`,
    kind: "event",
    list_id: `id-${name}`,
    name,
    view_id: view,
    anchor_position: at,
    anchor_label: at == null ? null : `X #${at}`,
    last_read_at: null,
  };
}

describe("orderParents", () => {
  const ps = [parent("A", "va"), parent("B", "vb"), parent("C", "vc")];
  it("keeps the server's order (most recently read first) by default", () => {
    expect(orderParents(ps, null).map((p) => p.name)).toEqual(["A", "B", "C"]);
  });
  it("puts the list the reader came from first", () => {
    expect(orderParents(ps, "vc").map((p) => p.name)).toEqual(["C", "A", "B"]);
  });
  it("ignores a via that names none of them", () => {
    expect(orderParents(ps, "zz").map((p) => p.name)).toEqual(["A", "B", "C"]);
  });
});

describe("parentHref", () => {
  it("opens the parent at the event's band", () => {
    expect(parentHref(parent("A", "va", 12), "va")).toBe("/views/va?event=l-A");
  });
  it("opens a part's predecessor plainly", () => {
    expect(
      parentHref({ ...parent("A", "va", null), kind: "continues" }, "va"),
    ).toBe("/views/va");
  });
});

describe("nextUpListSubtitle with a list switch", () => {
  const base = {
    source: "cbl",
    cbl_position: 4,
    cbl_total: 9,
    cbl_list_name: "Master",
  } as NextUpView;
  it("says where reading returns to", () => {
    expect(
      nextUpListSubtitle({
        ...base,
        switch_to: {
          reason: "return",
          list_id: "m",
          name: "[Marvel] Master Part #07 (WEB-CBRO)",
          view_id: "v",
        },
      }),
    ).toBe("Back to Master Part #07");
  });
  it("says the next part continues", () => {
    expect(
      nextUpListSubtitle({
        ...base,
        switch_to: {
          reason: "continues",
          list_id: "p",
          name: "Part 2",
          view_id: null,
        },
      }),
    ).toBe("Continue in Part 2");
  });
  it("keeps the in-list position otherwise", () => {
    expect(nextUpListSubtitle(base)).toBe("Issue 4 of 9 in Master");
  });
});
