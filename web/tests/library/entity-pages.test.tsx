/**
 * WP-5.5 entity landing pages — chip → landing-page linking and the
 * cursor contract the entity grids rely on.
 */
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { ChipList } from "@/components/library/ChipList";
import { entityNextPage } from "@/lib/api/queries";
import type { EntitySlugs } from "@/lib/api/types";
import { ENTITY_KINDS, entityHrefFor, entityUrl } from "@/lib/entities";

const slugs: EntitySlugs = {
  characters: { Batman: "batman", "Harley Quinn": "harley-quinn" },
  teams: { "Justice League": "justice-league" },
  arcs: { Knightfall: "knightfall" },
  publishers: { "DC Comics": "dc-comics" },
};

describe("entityHrefFor", () => {
  it("maps each chip field to its landing page", () => {
    expect(entityHrefFor("characters", "Batman", slugs)).toBe(
      "/characters/batman",
    );
    expect(entityHrefFor("teams", "Justice League", slugs)).toBe(
      "/teams/justice-league",
    );
    expect(entityHrefFor("story_arc", "Knightfall", slugs)).toBe(
      "/arcs/knightfall",
    );
    expect(entityHrefFor("publisher", "DC Comics", slugs)).toBe(
      "/publishers/dc-comics",
    );
  });

  it("returns null for unknown names, fields without pages, or no map", () => {
    expect(entityHrefFor("characters", "Robin", slugs)).toBeNull();
    expect(entityHrefFor("locations", "Gotham", slugs)).toBeNull();
    expect(entityHrefFor("characters", "Batman", undefined)).toBeNull();
  });

  it("encodes slugs", () => {
    expect(entityUrl("arcs", "a b")).toBe("/arcs/a%20b");
  });

  it("publishers are series-only", () => {
    expect(ENTITY_KINDS.publishers.hasIssues).toBe(false);
    expect(ENTITY_KINDS.arcs.hasIssues).toBe(true);
  });
});

describe("ChipList entity links", () => {
  it("links slugged names to the landing page, keeps the filter fallback", () => {
    const html = renderToStaticMarkup(
      createElement(ChipList, {
        items: ["Batman", "Robin"],
        filterField: "characters",
        entitySlugs: slugs,
      }),
    );
    expect(html).toContain('href="/characters/batman"');
    // No entity row yet → legacy issues-grid filter link.
    expect(html).toContain("mode=issues&amp;characters=Robin");
  });

  it("links story-arc chips (no library filter) only when slugged", () => {
    const html = renderToStaticMarkup(
      createElement(ChipList, {
        items: ["Knightfall", "Unknown Arc"],
        entityField: "story_arc",
        entitySlugs: slugs,
      }),
    );
    expect(html).toContain('href="/arcs/knightfall"');
    expect(html.match(/<a /g)?.length).toBe(1);
  });
});

describe("entityNextPage (entity grid cursor contract)", () => {
  it("forwards next_cursor and stops on null", () => {
    expect(entityNextPage({ next_cursor: "abc" })).toBe("abc");
    expect(entityNextPage({ next_cursor: null })).toBeUndefined();
    expect(entityNextPage({})).toBeUndefined();
  });
});
