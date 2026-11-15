// @vitest-environment jsdom
/**
 * Roadmap WP-9.10: the creator page's profile block and statistics, the
 * portrait with its monogram fallback, and the helpers that turn a
 * provider's text into a date and a safe link.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CreatorAvatar, monogram } from "@/components/library/CreatorAvatar";
import { InsightsBody } from "@/components/library/EntityInsights";
import {
  EntityProfile,
  factHref,
  factRows,
  formatFactDate,
  type ProfileSubject,
} from "@/components/library/EntityProfile";
import type { EntityInsightsView } from "@/lib/api/types";

const FACTS = {
  gender: "Male",
  born: "1956-06-18",
  died: "2020-01-02",
  country: "United Kingdom",
  hometown: "Corby",
  website: "https://www.example.com/art/",
  email: "alan@example.com",
  characters_created: ["Excalibur", "ClanDestine"],
  series_credits: 555,
  story_arc_credits: 12,
};

function subject(partial: Partial<ProfileSubject> = {}): ProfileSubject {
  return {
    image_url: "https://comicvine.gamespot.com/a/uploads/scale_medium/alan.jpg",
    description: "A British artist and writer.",
    aliases: ["A. Davis"],
    enrichment: "done",
    links: [
      {
        source: "comicvine",
        label: "ComicVine",
        external_id: "2348",
        url: "https://comicvine.gamespot.com/person/4040-2348/",
      },
    ],
    profile: {
      summary: "An English writer and artist of comic books.",
      sections: [],
      facts: FACTS,
      provider_counts: {},
      sources: ["metron", "comicvine"],
      fetched_at: "2026-11-15T00:00:00Z",
    },
    ...partial,
  };
}

describe("creator facts", () => {
  it("lists everything the provider holds, in biography order", () => {
    const rows = factRows(FACTS, "creators");
    expect(rows.map((r) => r.label)).toEqual([
      "Born",
      "Died",
      "Hometown",
      "Country",
      "Gender",
      "Website",
      "Email",
      "Series credited",
      "Story arcs credited",
    ]);
    expect(rows[0]!.value).toBe("June 18, 1956");
    expect(rows[5]).toEqual({
      label: "Website",
      value: "example.com/art",
      href: "https://www.example.com/art/",
    });
    expect(rows[6]!.href).toBe("mailto:alan@example.com");
    expect(rows[7]!.value).toBe("555 on record");
    // A character's rows are not a creator's.
    expect(factRows({ real_name: "x", ...FACTS }, "creators")).toHaveLength(9);
    expect(factRows({}, "creators")).toEqual([]);
  });

  it("formats a full date and leaves anything else as written", () => {
    expect(formatFactDate("1914-02-08")).toBe("February 8, 1914");
    expect(formatFactDate("1914")).toBe("1914");
    expect(formatFactDate("circa 1920")).toBe("circa 1920");
  });

  it("links only web sites and plain addresses", () => {
    expect(factHref("website", "http://example.com")).toBe(
      "http://example.com/",
    );
    expect(factHref("website", "javascript:alert(1)")).toBeNull();
    expect(factHref("website", "example.com")).toBeNull();
    expect(factHref("email", "a@b.co")).toBe("mailto:a@b.co");
    expect(factHref("email", "a@b.co?subject=x y")).toBeNull();
    expect(factHref("email", "not an address")).toBeNull();
  });
});

describe("EntityProfile on a creator page", () => {
  it("draws the portrait, summary, facts, created characters and links", () => {
    render(
      <EntityProfile kind="creators" detail={subject()} isAdmin={false} />,
    );
    expect(
      screen.getByText("An English writer and artist of comic books."),
    ).toBeTruthy();
    expect(screen.getByText("June 18, 1956")).toBeTruthy();
    expect(screen.getByText("Corby")).toBeTruthy();
    const mail = screen.getByRole("link", { name: "alan@example.com" });
    expect(mail.getAttribute("href")).toBe("mailto:alan@example.com");
    expect(screen.getByText("Characters created")).toBeTruthy();
    expect(screen.getByText("ClanDestine")).toBeTruthy();
    expect(screen.getByText("A. Davis")).toBeTruthy();
    expect(screen.getByText(/Filled from Metron \+ ComicVine/)).toBeTruthy();
    expect(
      screen.getByRole("link", { name: /ComicVine/ }).getAttribute("href"),
    ).toBe("https://comicvine.gamespot.com/person/4040-2348/");
    // No issue count from the provider, so no coverage line; and a reader
    // gets no Refresh.
    expect(screen.queryByText(/you own/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Refresh" })).toBeNull();
    expect(document.querySelector("img")?.getAttribute("src")).toContain(
      "alan.jpg",
    );
  });

  it("says a fetch is running, and offers an admin Refresh once it is not", () => {
    const { rerender } = render(
      <EntityProfile
        kind="creators"
        detail={subject({ enrichment: "pending", profile: undefined })}
        isAdmin
        onRefresh={() => {}}
      />,
    );
    expect(
      screen.getByText(/Fetching details from your metadata providers/),
    ).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    rerender(
      <EntityProfile
        kind="creators"
        detail={subject()}
        isAdmin
        onRefresh={() => {}}
      />,
    );
    expect(
      (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
  });

  it("is silent for a reader when nothing is cached", () => {
    const { container } = render(
      <EntityProfile
        kind="creators"
        detail={{
          image_url: null,
          description: null,
          aliases: [],
          enrichment: "unavailable",
        }}
        isAdmin={false}
      />,
    );
    expect(container.textContent).toBe("");
  });
});

describe("creator statistics", () => {
  const data: EntityInsightsView = {
    issues: 78,
    series: 9,
    first_year: 1986,
    last_year: 2019,
    read: 12,
    publishers: [{ name: "Marvel", count: 70 }],
    decades: [{ decade: 1980, count: 30 }],
    top_series: [{ name: "Excalibur", slug: "excalibur", count: 40 }],
    characters: [{ name: "Nightcrawler", slug: "nightcrawler", count: 38 }],
    teams: [],
    locations: [],
    arcs: [],
    creators: [
      { name: "Mark Farmer", slug: "mark-farmer", roles: "inker", count: 55 },
    ],
    earliest: [],
  };

  it("names the lists for a creator and links collaborators", () => {
    render(<InsightsBody kind="creators" data={data} />);
    expect(screen.getByText("In your library")).toBeTruthy();
    expect(screen.getByText("Frequent collaborators")).toBeTruthy();
    expect(screen.getByText("Characters in their issues")).toBeTruthy();
    expect(screen.queryByText("Most frequent creators")).toBeNull();
    expect(
      screen.getByRole("link", { name: "Mark Farmer" }).getAttribute("href"),
    ).toBe("/creators/mark-farmer");
    expect(screen.getByText("66 unread")).toBeTruthy();
  });
});

describe("CreatorAvatar", () => {
  it("is the monogram without a portrait", () => {
    render(<CreatorAvatar name="Fiona Staples" className="size-11" />);
    expect(screen.getByTestId("creator-monogram").textContent).toBe("FS");
    expect(document.querySelector("img")).toBeNull();
    expect(monogram("  cher ")).toBe("C");
  });

  it("shows the portrait, and falls back to the monogram when it fails", () => {
    render(
      <CreatorAvatar
        name="Fiona Staples"
        src="https://img.example/fs.jpg"
        className="size-11"
      />,
    );
    const img = document.querySelector("img")!;
    expect(img.getAttribute("src")).toBe("https://img.example/fs.jpg");
    expect(img.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(screen.queryByTestId("creator-monogram")).toBeNull();
    fireEvent.error(img);
    expect(screen.getByTestId("creator-monogram").textContent).toBe("FS");
    expect(document.querySelector("img")).toBeNull();
  });
});
