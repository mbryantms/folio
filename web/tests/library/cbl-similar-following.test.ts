/**
 * `similarFollowing` — the contiguous same-series run a CBL resolution row
 * offers to bulk-match in one click (B10). Guards the run boundaries: it
 * stops at the first differing series name and never reaches back.
 */
import { describe, expect, it } from "vitest";

import { similarFollowing } from "@/components/cbl/cbl-detail";
import type { CblEntryHydratedView } from "@/lib/api/types";

function e(id: string, series_name: string): CblEntryHydratedView {
  return { id, series_name } as unknown as CblEntryHydratedView;
}

describe("similarFollowing", () => {
  const items = [
    e("a", "Saga"),
    e("b", "Saga"),
    e("c", "Saga"),
    e("d", "Monstress"),
    e("e", "Saga"),
  ];

  it("returns the following same-name run, not the current entry", () => {
    expect(similarFollowing(items, 0).map((x) => x.id)).toEqual(["b", "c"]);
    expect(similarFollowing(items, 1).map((x) => x.id)).toEqual(["c"]);
  });

  it("stops at the first differing series name", () => {
    // index 2 is the last "Saga" before "Monstress" → no following run.
    expect(similarFollowing(items, 2)).toEqual([]);
  });

  it("a lone entry of its name has no run; the trailing one too", () => {
    expect(similarFollowing(items, 3)).toEqual([]); // Monstress, alone
    expect(similarFollowing(items, 4)).toEqual([]); // trailing Saga, last row
  });
});
