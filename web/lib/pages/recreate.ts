/**
 * Page-delete Undo support ("Undo instead of confirm", folio-flair-1.0).
 *
 * Deleting a custom page drops the row; the FK cascade removes its rail
 * pins and its sidebar override. The saved views themselves survive, so
 * everything the page *was* can be replayed through existing endpoints:
 *
 *   1. `POST /me/pages` with the old name (the slug is derived from the
 *      name and the old one is free again, so links keep working);
 *   2. `PATCH` the description back;
 *   3. re-pin each rail in its old order, then `reorder` to pin it down;
 *   4. hide it from the sidebar again if it was hidden (new pages show);
 *   5. put it back in its old slot among the user's pages.
 *
 * Like the collection undo ([`lib/collections/recreate.ts`]), the page
 * comes back with a fresh `id`. Nothing else references a page id.
 */
import { apiMutate } from "@/lib/api/mutations/_core";
import { jsonFetch } from "@/lib/api/queries";
import type { PageView, SavedViewListView } from "@/lib/api/types";

export type PageSnapshot = {
  id: string;
  name: string;
  description: string | null;
  showInSidebar: boolean;
  /** Rail (saved view) ids pinned to the page, in page order. */
  pinnedViewIds: string[];
  /** Every page id the user owned, in stored order (incl. this one). */
  pageOrder: string[];
};

/** Capture what [`recreatePage`] needs, *before* the delete. `pages` is
 *  the user's page list (the `useMePages` cache); it's fetched when the
 *  caller doesn't have it. The rail list is always fetched fresh — it is
 *  the page's content and must not come from a stale cache. */
export async function snapshotPage(
  pageId: string,
  pages?: readonly PageView[],
): Promise<PageSnapshot> {
  const all =
    pages && pages.length > 0
      ? pages
      : (await jsonFetch<{ items: PageView[] }>("/me/pages")).items;
  const page = all.find((p) => p.id === pageId);
  if (!page) throw new Error("page not found");
  const rails = await jsonFetch<SavedViewListView>(
    `/me/saved-views?pinned_on=${encodeURIComponent(pageId)}`,
  );
  return {
    id: page.id,
    name: page.name,
    description: page.description ?? null,
    showInSidebar: page.show_in_sidebar,
    pinnedViewIds: rails.items.map((v) => v.id),
    pageOrder: [...all]
      .sort((a, b) => a.position - b.position)
      .map((p) => p.id),
  };
}

/** Replay a snapshot. Returns the new page. Throws when the page itself
 *  can't be created; later steps are best-effort per rail (a view
 *  deleted since can't be re-pinned) but the arrangement calls throw so
 *  the Undo toast reports a partial restore honestly. */
export async function recreatePage(snap: PageSnapshot): Promise<PageView> {
  const created = await apiMutate<PageView>({
    path: "/me/pages",
    method: "POST",
    body: { name: snap.name },
  });
  if (!created) throw new Error("create returned nothing");
  if (snap.description) {
    await apiMutate({
      path: `/me/pages/${created.id}`,
      method: "PATCH",
      body: { description: snap.description },
    });
  }
  const pinned: string[] = [];
  // Sequential: each pin appends at the page's next position, so this
  // already lays the rails out in their old order.
  for (const viewId of snap.pinnedViewIds) {
    try {
      await apiMutate({
        path: `/me/saved-views/${viewId}/pin`,
        method: "POST",
        body: { page_ids: [created.id] },
      });
      pinned.push(viewId);
    } catch {
      // The view was deleted since; nothing to pin.
    }
  }
  if (pinned.length > 1) {
    await apiMutate({
      path: "/me/saved-views/reorder",
      method: "POST",
      body: { page_id: created.id, view_ids: pinned },
    });
  }
  if (!snap.showInSidebar && created.show_in_sidebar) {
    await apiMutate({
      path: `/me/pages/${created.id}/sidebar?show=false`,
      method: "POST",
    });
  }
  // Old slot among the pages. Pages created or deleted since are
  // reconciled against the live list so the reorder lists every owned
  // page exactly once (the server rejects anything else).
  const live = (await jsonFetch<{ items: PageView[] }>("/me/pages")).items;
  const liveIds = new Set(live.map((p) => p.id));
  const order = snap.pageOrder
    .map((id) => (id === snap.id ? created.id : id))
    .filter((id) => liveIds.has(id));
  for (const p of [...live].sort((a, b) => a.position - b.position)) {
    if (!order.includes(p.id)) order.push(p.id);
  }
  await apiMutate({
    path: "/me/pages/reorder",
    method: "POST",
    body: { page_ids: order },
  });
  return { ...created, show_in_sidebar: snap.showInSidebar };
}
