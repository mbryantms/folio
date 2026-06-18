/**
 * Collection-delete Undo support (audit B6).
 *
 * Deleting a collection drops the row and every membership entry — the
 * underlying series/issues are untouched, but the hand-curated list is
 * gone. To make that reversible, the delete call sites snapshot the
 * collection (name + description + ordered members) *before* firing the
 * delete, then an Undo toast replays the snapshot via
 * [`recreateCollection`].
 *
 * Like the marker-undo path ([`lib/markers/recreate.ts`]), Undo restores
 * the *content*, not the row: the recreated collection gets a fresh `id`
 * and `created_at`. Pins / sidebar visibility are deliberately NOT
 * restored — they're arrangement, not content, and a brand-new id can't
 * inherit the old row's placement anyway.
 */
import { apiMutate } from "@/lib/api/mutations/_core";
import { jsonFetch } from "@/lib/api/queries";
import type {
  CollectionEntriesView,
  CollectionEntryView,
  SavedViewView,
} from "@/lib/api/types";

export type CollectionMember = {
  entry_kind: "issue" | "series";
  ref_id: string;
};

export type CollectionSnapshot = {
  name: string;
  description: string | null;
  members: CollectionMember[];
};

/** Project a hydrated entry to the `(entry_kind, ref_id)` member the
 *  bulk-add endpoint expects. Mirrors the `selectedTargets` resolution on
 *  the collection detail page. Null when the entry's hydrated side is
 *  missing (a dangling row) so the snapshot stays faithful. */
export function entryToMember(e: CollectionEntryView): CollectionMember | null {
  if (e.entry_kind === "issue" && e.issue) {
    return { entry_kind: "issue", ref_id: e.issue.id };
  }
  if (e.entry_kind === "series" && e.series) {
    return { entry_kind: "series", ref_id: e.series.id };
  }
  return null;
}

/** Build a snapshot from already-loaded, position-ordered entries. The
 *  detail page auto-walks every page, so its list is complete — no extra
 *  fetch needed. */
export function snapshotFromEntries(
  name: string,
  description: string | null | undefined,
  entries: readonly CollectionEntryView[],
): CollectionSnapshot {
  return {
    name,
    description: description ?? null,
    members: entries
      .map(entryToMember)
      .filter((m): m is CollectionMember => m !== null),
  };
}

/** Cap on snapshot pages walked / members replayed. 30 pages × 200 ≈ 6k
 *  members; far past any hand-curated list, but bounded so a pathological
 *  collection can't loop or promise an unreplayable undo. */
const MAX_SNAPSHOT_PAGES = 30;

/** Walk every entry page for a collection and build a snapshot. Used by
 *  callers (the views-index card) that don't already hold the entries.
 *  Runs *before* the delete so the rows are still there to read. */
export async function fetchCollectionSnapshot(
  id: string,
  name: string,
  description: string | null | undefined,
): Promise<CollectionSnapshot> {
  const entries: CollectionEntryView[] = [];
  let cursor: string | undefined;
  for (let page = 0; page < MAX_SNAPSHOT_PAGES; page++) {
    const q = new URLSearchParams({ limit: "200" });
    if (cursor) q.set("cursor", cursor);
    const view = await jsonFetch<CollectionEntriesView>(
      `/me/collections/${id}/entries?${q.toString()}`,
    );
    entries.push(...view.items);
    if (!view.next_cursor) break;
    cursor = view.next_cursor;
  }
  return snapshotFromEntries(name, description, entries);
}

/** Per-call member cap on the bulk-add endpoint (server-enforced at 500). */
const BULK_ADD_CAP = 500;

/** Recreate a deleted collection from its snapshot: create the row, then
 *  bulk-add its members in their original order. The server's bulk-add
 *  walks the position counter forward per insert, so submitting members in
 *  snapshot order preserves the original arrangement without a separate
 *  reorder round-trip. Members are chunked at the server's 500-per-call
 *  cap. Returns the recreated collection (new id), or null if the create
 *  failed. */
export async function recreateCollection(
  snap: CollectionSnapshot,
): Promise<SavedViewView | null> {
  const created = await apiMutate<SavedViewView>({
    path: "/me/collections",
    method: "POST",
    body: { name: snap.name, description: snap.description ?? undefined },
  });
  if (!created) return null;
  for (let i = 0; i < snap.members.length; i += BULK_ADD_CAP) {
    const chunk = snap.members.slice(i, i + BULK_ADD_CAP);
    await apiMutate({
      path: `/me/collections/${created.id}/members/bulk-add`,
      method: "POST",
      body: { members: chunk },
    });
  }
  return created;
}
