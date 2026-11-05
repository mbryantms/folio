/**
 * Page-delete Undo replays the snapshot: name, description, rails in
 * order, hidden-from-sidebar, and the page's old slot.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  apiMutate: vi.fn(),
  jsonFetch: vi.fn(),
}));
vi.mock("@/lib/api/mutations/_core", () => ({ apiMutate: m.apiMutate }));
vi.mock("@/lib/api/queries", () => ({ jsonFetch: m.jsonFetch }));

import type { PageView } from "@/lib/api/types";
import { recreatePage, snapshotPage } from "@/lib/pages/recreate";

const page = (id: string, position: number, extra = {}): PageView =>
  ({
    id,
    name: id.toUpperCase(),
    slug: id,
    position,
    is_system: id === "home",
    show_in_sidebar: true,
    pin_count: 0,
    description: null,
    created_at: "",
    updated_at: "",
    ...extra,
  }) as PageView;

beforeEach(() => {
  m.apiMutate.mockReset();
  m.jsonFetch.mockReset();
});

describe("page delete undo", () => {
  it("snapshots and replays a page in its old slot", async () => {
    const pages = [
      page("home", 0),
      page("b", 2, {
        description: "Weekly pulls",
        show_in_sidebar: false,
      }),
      page("a", 1),
    ];
    m.jsonFetch.mockResolvedValueOnce({ items: [{ id: "v2" }, { id: "v1" }] });
    const snap = await snapshotPage("b", pages);
    expect(snap).toEqual({
      id: "b",
      name: "B",
      description: "Weekly pulls",
      showInSidebar: false,
      pinnedViewIds: ["v2", "v1"],
      pageOrder: ["home", "a", "b"],
    });

    m.apiMutate.mockImplementation(async ({ path, method }) =>
      path === "/me/pages" && method === "POST"
        ? page("b2", 3, { slug: "b" })
        : null,
    );
    // Live list after the re-create: the new page sits at the tail.
    m.jsonFetch.mockResolvedValueOnce({
      items: [page("home", 0), page("a", 1), page("b2", 3)],
    });
    const created = await recreatePage(snap);
    expect(created.slug).toBe("b");
    const calls = m.apiMutate.mock.calls.map(([c]) => [
      c.method,
      c.path,
      c.body,
    ]);
    expect(calls).toEqual([
      ["POST", "/me/pages", { name: "B" }],
      ["PATCH", "/me/pages/b2", { description: "Weekly pulls" }],
      ["POST", "/me/saved-views/v2/pin", { page_ids: ["b2"] }],
      ["POST", "/me/saved-views/v1/pin", { page_ids: ["b2"] }],
      [
        "POST",
        "/me/saved-views/reorder",
        { page_id: "b2", view_ids: ["v2", "v1"] },
      ],
      ["POST", "/me/pages/b2/sidebar?show=false", undefined],
      ["POST", "/me/pages/reorder", { page_ids: ["home", "a", "b2"] }],
    ]);
  });
});
