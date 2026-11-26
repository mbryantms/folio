/**
 * Linked reading lists 2.0 PR 5: reorder among events at one issue, the
 * lost-anchor list, and the delete confirmation's note.
 */
import { describe, expect, it } from "vitest";

import {
  anchorSiblings,
  anchorWasAfter,
  lostAnchors,
  reorderWrites,
} from "@/components/cbl/links/MoveEventDialog";
import { deleteLinkedNote, refreshLinkNote } from "@/components/cbl/cbl-detail";
import type {
  ReadingListBreakView,
  ReadingListLinkView,
  ReadingListParentView,
} from "@/lib/api/types";

function ev(
  id: string,
  position: number,
  sort: number,
  status: "placed" | "lost" = "placed",
): ReadingListLinkView {
  return {
    id,
    kind: "event",
    child_list_id: `list-${id}`,
    child_name: `[Marvel] Event ${id} (WEB-CBRO)`,
    sort_within_anchor: sort,
    anchor: { position, status, series: "Avengers", number: "12" },
  } as unknown as ReadingListLinkView;
}

describe("anchorSiblings / reorderWrites", () => {
  const a = ev("a", 5, 0);
  const b = ev("b", 5, 1);
  const c = ev("c", 5, 2);
  const other = ev("z", 9, 0);

  it("lists the events after the same issue, only when several", () => {
    expect(anchorSiblings([a, b, c, other], b).map((e) => e.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
    expect(anchorSiblings([a, other], other)).toEqual([]);
    expect(anchorSiblings([a, b], ev("l", 5, 0, "lost"))).toEqual([]);
  });

  it("swaps one step and writes only the slots that change", () => {
    expect(reorderWrites([a, b, c], b, -1)).toEqual([
      { linkId: "b", sort_within_anchor: 0 },
      { linkId: "a", sort_within_anchor: 1 },
    ]);
    expect(reorderWrites([a, b, c], c, 1)).toEqual([]);
    expect(reorderWrites([a, b, c], a, -1)).toEqual([]);
  });

  it("still changes the order when the stored values tie", () => {
    const x = ev("x", 5, 0);
    const y = ev("y", 5, 0);
    // y moves before x: y keeps 0, x must become 1.
    expect(reorderWrites([x, y], y, -1)).toEqual([
      { linkId: "x", sort_within_anchor: 1 },
    ]);
  });
});

describe("lost anchors", () => {
  it("picks the lost events and breaks", () => {
    const breaks = [
      { id: "b1", label: "Act 2", anchor: { status: "lost" } },
      { id: "b2", label: null, anchor: { status: "placed" } },
    ] as unknown as ReadingListBreakView[];
    const got = lostAnchors([ev("a", 1, 0), ev("b", 2, 0, "lost")], breaks);
    expect(got.events.map((e) => e.id)).toEqual(["b"]);
    expect(got.breaks.map((b) => b.id)).toEqual(["b1"]);
  });
  it("words where an anchor was", () => {
    expect(anchorWasAfter({ series: "Excalibur", number: "12" })).toBe(
      "Was after Excalibur #12",
    );
    expect(anchorWasAfter({ series: null })).toBeNull();
    expect(anchorWasAfter(null)).toBeNull();
  });
});

describe("refreshLinkNote", () => {
  it("reports lost and restored linked items, or nothing", () => {
    expect(refreshLinkNote({ links_lost: 2, links_restored: 1 })).toBe(
      " · 2 linked items lost their place · 1 back in place",
    );
    expect(refreshLinkNote({ links_lost: 1 })).toBe(
      " · 1 linked item lost its place",
    );
    expect(refreshLinkNote({ added: [] })).toBe("");
    expect(refreshLinkNote(null)).toBe("");
  });
});

describe("deleteLinkedNote", () => {
  const parent = (name: string) => ({ name }) as ReadingListParentView;
  it("is null without links", () => {
    expect(
      deleteLinkedNote({ events: [], continues: null, parents: [] }, false),
    ).toBeNull();
    expect(deleteLinkedNote(undefined, false)).toBeNull();
  });
  it("names the lists it is linked into and the lists linked into it", () => {
    const note = deleteLinkedNote(
      {
        events: [ev("a", 1, 0)],
        continues: null,
        parents: [parent("[Marvel] Master Part #1 (WEB-CBRO)")],
      },
      true,
    );
    expect(note).toContain("It is linked into");
    expect(note).toContain("for every reader");
    expect(note).toContain("is linked into it; that list stays");
  });
});
