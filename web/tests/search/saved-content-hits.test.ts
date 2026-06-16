/** A4 palette sources: the `views` / `collections` / `pages` categories
 *  are name-filtered client-side over the cached `/me/saved-views`,
 *  `/me/collections`, and `/me/pages` lists. These tests pin the
 *  categorization rules — kind partitioning, the Want-to-Read href
 *  alias, and the empty-needle short-circuit — without rendering the
 *  hook. */
import { describe, expect, it } from "vitest";

import {
  buildSavedContentHits,
  type SavedContentSources,
} from "@/lib/search/use-search";
import type { PageView, SavedViewView } from "@/lib/api/types";

function view(partial: Partial<SavedViewView>): SavedViewView {
  return {
    id: "v1",
    name: "View",
    kind: "filter_series",
    is_system: false,
    pinned: false,
    pinned_on_pages: [],
    show_in_sidebar: false,
    ...partial,
  } as SavedViewView;
}

function page(partial: Partial<PageView>): PageView {
  return {
    id: "p1",
    name: "Page",
    slug: "page",
    is_system: false,
    show_in_sidebar: false,
    position: 0,
    pin_count: 0,
    ...partial,
  } as PageView;
}

const SOURCES: SavedContentSources = {
  savedViews: [
    view({ id: "f1", name: "Marvel Heroes", kind: "filter_series" }),
    view({ id: "c1", name: "Marvel Reading Order", kind: "cbl" }),
    view({ id: "s1", name: "Marvel Continue", kind: "system" }),
    // A `collection`-kind row leaking through `/me/saved-views` must not
    // double-count into the `views` category — collections have their
    // own source.
    view({ id: "x1", name: "Marvel Collection", kind: "collection" }),
    view({ id: "f2", name: "DC Villains", kind: "filter_series" }),
  ],
  collections: [
    view({
      id: "wtr",
      name: "Want to Read",
      kind: "collection",
      system_key: "want_to_read",
    }),
    view({ id: "col1", name: "Marvel Faves", kind: "collection" }),
  ],
  pages: [
    page({ id: "home", name: "Home", slug: "home" }),
    page({ id: "mp", name: "Marvel Picks", slug: "marvel-picks" }),
  ],
};

describe("buildSavedContentHits", () => {
  it("returns empty groups for an empty needle", () => {
    expect(buildSavedContentHits("", SOURCES)).toEqual({
      views: [],
      collections: [],
      pages: [],
    });
  });

  it("keeps only filter_series + cbl in views (drops system + collection)", () => {
    const { views } = buildSavedContentHits("marvel", SOURCES);
    expect(views.map((h) => h.id)).toEqual(["f1", "c1"]);
    // Subtitles distinguish the two kinds per the /views concept copy.
    expect(views.map((h) => h.subtitle)).toEqual([
      "Filter view",
      "Reading list",
    ]);
    // Every hit carries the right discriminator + href shape.
    expect(views.every((h) => h.kind === "views")).toBe(true);
    expect(views[0]!.href).toBe("/views/f1");
  });

  it("name-matches case-insensitively across all three categories", () => {
    const { views, collections, pages } = buildSavedContentHits(
      "marvel",
      SOURCES,
    );
    expect(views).toHaveLength(2);
    expect(collections.map((h) => h.id)).toEqual(["col1"]);
    expect(pages.map((h) => h.id)).toEqual(["mp"]);
    expect(pages[0]!.href).toBe("/pages/marvel-picks");
  });

  it("routes Want to Read through the want-to-read alias", () => {
    const { collections } = buildSavedContentHits("want", SOURCES);
    expect(collections).toHaveLength(1);
    expect(collections[0]!.href).toBe("/views/want-to-read");
    expect(collections[0]!.subtitle).toBe("Built-in collection");
  });

  it("uses the collection id for non-system collections", () => {
    const { collections } = buildSavedContentHits("faves", SOURCES);
    expect(collections[0]!.href).toBe("/views/col1");
    expect(collections[0]!.subtitle).toBe("Collection");
  });
});
