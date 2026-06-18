import { describe, expect, it } from "vitest";

import { entryToMember, snapshotFromEntries } from "@/lib/collections/recreate";
import type { CollectionEntryView } from "@/lib/api/types";

/** Minimal `CollectionEntryView` factory — the projection only reads
 *  `entry_kind` + the hydrated `series`/`issue` id, so the rest is cast. */
function entry(
  partial: Partial<CollectionEntryView> & Pick<CollectionEntryView, "id">,
): CollectionEntryView {
  return {
    added_at: "2026-01-01T00:00:00Z",
    entry_kind: "series",
    position: 0,
    ...partial,
  } as CollectionEntryView;
}

describe("entryToMember", () => {
  it("maps a series entry to its series ref_id", () => {
    const e = entry({
      id: "row-1",
      entry_kind: "series",
      series: { id: "ser-9" } as CollectionEntryView["series"],
    });
    expect(entryToMember(e)).toEqual({ entry_kind: "series", ref_id: "ser-9" });
  });

  it("maps an issue entry to its issue ref_id", () => {
    const e = entry({
      id: "row-2",
      entry_kind: "issue",
      issue: { id: "iss-3" } as CollectionEntryView["issue"],
    });
    expect(entryToMember(e)).toEqual({ entry_kind: "issue", ref_id: "iss-3" });
  });

  it("returns null for a dangling entry with no hydrated side", () => {
    expect(
      entryToMember(entry({ id: "row-3", entry_kind: "series" })),
    ).toBeNull();
    expect(
      entryToMember(entry({ id: "row-4", entry_kind: "issue" })),
    ).toBeNull();
  });
});

describe("snapshotFromEntries", () => {
  it("preserves order and drops dangling rows", () => {
    const snap = snapshotFromEntries("My list", "desc", [
      entry({
        id: "r1",
        entry_kind: "series",
        series: { id: "s1" } as CollectionEntryView["series"],
      }),
      entry({ id: "r2", entry_kind: "issue" }), // dangling → dropped
      entry({
        id: "r3",
        entry_kind: "issue",
        issue: { id: "i1" } as CollectionEntryView["issue"],
      }),
    ]);
    expect(snap).toEqual({
      name: "My list",
      description: "desc",
      members: [
        { entry_kind: "series", ref_id: "s1" },
        { entry_kind: "issue", ref_id: "i1" },
      ],
    });
  });

  it("normalizes a missing description to null", () => {
    expect(snapshotFromEntries("x", undefined, []).description).toBeNull();
    expect(snapshotFromEntries("x", null, []).description).toBeNull();
  });
});
