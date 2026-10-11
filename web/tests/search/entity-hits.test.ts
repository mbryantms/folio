/** The four comic-content search categories (characters / teams /
 *  locations / story arcs) are the entity browse indexes filtered by
 *  `q`. These tests pin the row → hit mapping (landing-page href, icon
 *  per kind, appearance-count subtitle) and that the taxonomy lists
 *  every category the hook emits, so the modal and `/search` render
 *  them without further wiring. */
import { describe, expect, it } from "vitest";

import type { EntityListItem } from "@/lib/api/types";
import { entityToSearchHit } from "@/lib/search/use-search";
import {
  EMPTY_SEARCH_GROUPS,
  SEARCH_CATEGORIES,
  flattenGroups,
} from "@/lib/search/types";

function item(partial: Partial<EntityListItem>): EntityListItem {
  return {
    id: "e1",
    slug: "gotham-city",
    name: "Gotham City",
    series_count: 3,
    issue_count: 12,
    ...partial,
  };
}

describe("entityToSearchHit", () => {
  it("links each kind to its landing page with an appearance subtitle", () => {
    const hit = entityToSearchHit("locations", item({}));
    expect(hit.kind).toBe("locations");
    expect(hit.href).toBe("/locations/gotham-city");
    expect(hit.title).toBe("Gotham City");
    expect(hit.subtitle).toBe("12 issues · 3 series");
    expect(hit.icon).toBeDefined();
    expect(entityToSearchHit("characters", item({ slug: "batman" })).href).toBe(
      "/characters/batman",
    );
    expect(entityToSearchHit("teams", item({ slug: "a b" })).href).toBe(
      "/teams/a%20b",
    );
    expect(entityToSearchHit("arcs", item({ slug: "hush" })).href).toBe(
      "/arcs/hush",
    );
  });

  it("uses a distinct icon per kind and singular/series-only wording", () => {
    const icons = new Set(
      (["characters", "teams", "locations", "arcs"] as const).map(
        (k) => entityToSearchHit(k, item({})).icon,
      ),
    );
    expect(icons.size).toBe(4);
    expect(entityToSearchHit("teams", item({ issue_count: 1 })).subtitle).toBe(
      "1 issue · 3 series",
    );
    // A series-level-only membership has no issue hits to count.
    expect(
      entityToSearchHit("teams", item({ issue_count: 0, series_count: 1 }))
        .subtitle,
    ).toBe("1 series");
  });
});

describe("entity categories in the taxonomy", () => {
  it("lists the four entity categories after people and before saved content", () => {
    const keys = SEARCH_CATEGORIES.map((c) => c.key);
    expect(keys.indexOf("characters")).toBe(keys.indexOf("people") + 1);
    expect(
      keys.slice(keys.indexOf("characters"), keys.indexOf("views")),
    ).toEqual(["characters", "teams", "locations", "arcs"]);
  });

  it("flattens entity groups in display order", () => {
    const groups = {
      ...EMPTY_SEARCH_GROUPS,
      arcs: [entityToSearchHit("arcs", item({ id: "a" }))],
      characters: [entityToSearchHit("characters", item({ id: "c" }))],
      people: [
        { kind: "people" as const, id: "p", title: "Bob Kane", href: "/x" },
      ],
    };
    expect(flattenGroups(groups).map((h) => h.id)).toEqual(["p", "c", "a"]);
  });
});
