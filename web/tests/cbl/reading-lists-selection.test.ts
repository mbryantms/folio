/**
 * The reading-lists board's select mode: which lists can be ticked and
 * the wording of the confirm and the result.
 */
import { describe, expect, it } from "vitest";

import {
  bulkDeleteResult,
  bulkDeleteTitle,
  canDeleteList,
  deleteTakesList,
} from "@/components/cbl/ReadingListsSelection";
import type { SavedViewView } from "@/lib/api/types";

const view = (over: Partial<SavedViewView>): SavedViewView =>
  ({
    id: "v",
    kind: "cbl",
    name: "Infinity",
    is_system: false,
    ...over,
  }) as SavedViewView;

describe("canDeleteList", () => {
  it("is the reader's own lists only", () => {
    expect(canDeleteList(view({}))).toBe(true);
    expect(canDeleteList(view({ is_system: true }))).toBe(false);
    expect(canDeleteList(view({ kind: "collection" }))).toBe(false);
  });
});

describe("bulkDeleteTitle", () => {
  it("names one list and counts several", () => {
    expect(bulkDeleteTitle([view({})])).toBe('Delete "Infinity"?');
    expect(bulkDeleteTitle([view({}), view({ id: "w" })])).toBe(
      "Delete 2 reading lists?",
    );
  });
});

describe("bulkDeleteResult", () => {
  it("reports what went and what stayed", () => {
    expect(bulkDeleteResult(3, 0)).toEqual({
      ok: true,
      message: "Deleted 3 reading lists",
    });
    expect(bulkDeleteResult(1, 0).message).toBe("Deleted 1 reading list");
    expect(bulkDeleteResult(2, 1)).toEqual({
      ok: false,
      message: "Deleted 2 reading lists; 1 could not be deleted",
    });
    expect(bulkDeleteResult(0, 2)).toEqual({
      ok: false,
      message: "Couldn't delete 2 reading lists",
    });
  });
});

describe("deleteTakesList", () => {
  const cbl = view({ cbl_list_id: "l1" });
  const lists = (owner: string | null) =>
    new Map([["l1", { owner_user_id: owner }]]);
  it("is true for the reader's own list", () => {
    expect(deleteTakesList(cbl, lists("me"), "me")).toBe(true);
  });
  it("is false for a view of a list someone else shares", () => {
    expect(deleteTakesList(cbl, lists("other"), "me")).toBe(false);
    expect(deleteTakesList(cbl, lists(null), "me")).toBe(false);
  });
  it("assumes the list goes while ownership is unknown", () => {
    expect(deleteTakesList(cbl, null, "me")).toBe(true);
    expect(deleteTakesList(cbl, new Map(), "me")).toBe(true);
    expect(deleteTakesList(cbl, lists("me"), undefined)).toBe(true);
  });
  it("is false for anything that is not a reading list", () => {
    expect(
      deleteTakesList(view({ kind: "filter_series" }), lists("me"), "me"),
    ).toBe(false);
  });
});
