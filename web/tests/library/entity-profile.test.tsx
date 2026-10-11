/**
 * Entity landing-page profile block (provider-filled "wiki") and the
 * "In your library" insights: the pure helpers that shape the copy, and
 * the static markup of each enrichment state.
 */
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { decadeBars } from "@/components/library/EntityInsights";
import {
  EntityProfile,
  EntitySections,
  coverageLine,
  factRows,
} from "@/components/library/EntityProfile";
import type { EntityDetailView } from "@/lib/api/types";

function detail(partial: Partial<EntityDetailView>): EntityDetailView {
  return {
    kind: "characters",
    id: "c1",
    slug: "batman",
    name: "Batman",
    description: null,
    image_url: null,
    aliases: [],
    series_count: 780,
    issue_count: 6147,
    links: [],
    enrichment: "unavailable",
    ...partial,
  } as EntityDetailView;
}

describe("coverageLine", () => {
  it("uses the provider with the largest count and rounds the share", () => {
    expect(coverageLine({ metron: 9680, comicvine: 25186 }, 6147)).toBe(
      "ComicVine lists 25,186 issues · you own 6,147 of them (24%)",
    );
    expect(coverageLine({ metron: 100 }, 250)).toContain("(100%)");
    expect(coverageLine({}, 10)).toBeNull();
    expect(coverageLine({ metron: 0 }, 10)).toBeNull();
  });
});

describe("factRows", () => {
  it("lists only the facts a record carries, in display order", () => {
    const rows = factRows(
      {
        real_name: "Bruce Wayne",
        creators: ["Bob Kane", "Bill Finger"],
        first_appearance: { label: "Detective Comics #27" },
        publisher: "DC Comics",
        gender: "Male",
        members: ["Robin"],
      },
      "characters",
    );
    expect(rows.map((r) => r.label)).toEqual([
      "Real name",
      "Created by",
      "First appearance",
      "Publisher",
      "Gender",
    ]);
    expect(rows[1]!.value).toBe("Bob Kane, Bill Finger");
    // Members only make a row on team pages.
    expect(
      factRows({ members: ["Robin", "Batgirl"] }, "teams").map((r) => r.label),
    ).toEqual(["Members"]);
  });
});

describe("decadeBars", () => {
  it("scales to the busiest decade with a visible floor", () => {
    const bars = decadeBars([
      { decade: 1930, count: 8 },
      { decade: 2010, count: 2179 },
    ]);
    expect(bars[1]!.height).toBe(1);
    expect(bars[0]!.height).toBeCloseTo(0.02);
    expect(decadeBars([])).toEqual([]);
  });
});

describe("EntityProfile states", () => {
  const render = (d: EntityDetailView, isAdmin = false) =>
    renderToStaticMarkup(
      createElement(EntityProfile, {
        kind: "characters",
        detail: d,
        isAdmin,
        onRefresh: () => {},
      }),
    );

  it("says a fetch is running while pending", () => {
    const html = render(detail({ enrichment: "pending" }));
    expect(html).toContain("Fetching details from your metadata providers");
  });

  it("shows the recorded reason when the fetch failed", () => {
    const html = render(
      detail({
        enrichment: "failed",
        enrichment_error: 'Metron has no record named "Batman"',
      }),
    );
    expect(html).toContain("Couldn");
    expect(html).toContain("Metron has no record");
  });

  it("renders the filled profile with facts, chips, sources and links", () => {
    const html = render(
      detail({
        image_url: "https://static.metron.cloud/media/character/batman.jpg",
        aliases: ["Bruce Wayne"],
        enrichment: "done",
        links: [
          {
            source: "metron",
            label: "Metron",
            external_id: "12",
            url: "https://metron.cloud/character/batman/",
          },
        ],
        profile: {
          summary: "Batman is a fictional superhero.",
          sections: [],
          facts: {
            real_name: "Bruce Wayne",
            creators: ["Bob Kane"],
            powers: ["Intellect"],
            universes: ["Prime Earth"],
          },
          provider_counts: { metron: 9680 },
          sources: ["metron", "comicvine"],
          fetched_at: "2026-10-10T00:00:00Z",
        },
      }),
      true,
    );
    expect(html).toContain("Batman is a fictional superhero.");
    expect(html).toContain("Real name");
    expect(html).toContain("Intellect");
    expect(html).toContain("Prime Earth");
    expect(html).toContain("Filled from Metron + ComicVine");
    expect(html).toContain("Metron lists 9,680 issues");
    expect(html).toContain('href="https://metron.cloud/character/batman/"');
    expect(html).toContain("Refresh");
  });

  it("points at the admin provider settings when nothing can fill it", () => {
    const html = render(detail({ enrichment: "unavailable" }));
    expect(html).toContain("Add a metadata provider");
    // Non-admins never see the refresh control.
    expect(html).not.toContain("Refresh");
  });
});

describe("EntitySections", () => {
  it("opens the lead and first section, chips the rest", () => {
    const html = renderToStaticMarkup(
      createElement(EntitySections, {
        sections: [
          { heading: "", level: 0, paragraphs: ["Lead."] },
          { heading: "Origin", level: 2, paragraphs: ["Park Row."] },
          { heading: "Golden Age", level: 2, paragraphs: ["Bob Kane."] },
          { heading: "Knightfall", level: 3, paragraphs: ["Bane."] },
        ],
      }),
    );
    expect(html).toContain("Lead.");
    expect(html).toContain("Park Row.");
    expect(html).not.toContain("Bob Kane.");
    expect(html).toContain("Golden Age");
    expect(html).toContain("Knightfall");
    expect(html).toContain("3 sections");
  });
});
