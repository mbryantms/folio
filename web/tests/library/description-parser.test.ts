import { describe, expect, it } from "vitest";

import { parseDescription } from "@/lib/description-parser";

describe("parseDescription", () => {
  it("returns empty defaults for null/empty input", () => {
    expect(parseDescription(null)).toMatchObject({
      intro: "",
      tables: [],
      sections: [],
      hasStructuredContent: false,
    });
    expect(parseDescription("")).toMatchObject({ hasStructuredContent: false });
    expect(parseDescription("   \n  ")).toMatchObject({
      hasStructuredContent: false,
    });
  });

  it("returns plain prose untouched when no markers present", () => {
    const text = "Just a normal description with no tables or structure.";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(false);
    expect(parsed.intro).toBe(text);
    expect(parsed.tables).toHaveLength(0);
    expect(parsed.sections).toHaveLength(0);
  });

  it("ignores stray italic emphasis without a colon-marker + table", () => {
    const text = "Some prose with *random emphasis* in the middle.";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(false);
    expect(parsed.intro).toBe(text);
  });

  it("parses example 1 — short cover-list table", () => {
    const text =
      "ONE MORE DAY! *List of covers and their creators:* Cover | Name | Creator(s) | Sidebar Location | -------------------------------------------------------- Reg | Regular Cover | Chip Zdarsky | 1 | Var | Variant Cover | Declan Shalvey | 2 |";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    expect(parsed.intro).toBe("ONE MORE DAY!");
    expect(parsed.tables).toHaveLength(1);
    const table = parsed.tables[0];
    expect(table.title).toBe("List of covers and their creators");
    expect(table.columns).toEqual([
      "Cover",
      "Name",
      "Creator(s)",
      "Sidebar Location",
    ]);
    expect(table.rows).toEqual([
      ["Reg", "Regular Cover", "Chip Zdarsky", "1"],
      ["Var", "Variant Cover", "Declan Shalvey", "2"],
    ]);
    expect(parsed.sections).toHaveLength(0);
  });

  it("parses example 2 — long table followed by a *Notes* section", () => {
    const text =
      "The Greatest Super Hero of All Time RETURNS! *List of covers and their creators:* Cover | Name | Creator(s) | Sidebar Location | -------- Reg | Regular Cover | Humberto Ramos | 1 | Var | Variant Cover | Pop Mhan | 40 | *Notes* J. Scott Campbell's variant cover connects with Superior Spider-Man #31.";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    expect(parsed.intro).toBe("The Greatest Super Hero of All Time RETURNS!");
    expect(parsed.tables).toHaveLength(1);
    expect(parsed.tables[0].rows).toEqual([
      ["Reg", "Regular Cover", "Humberto Ramos", "1"],
      ["Var", "Variant Cover", "Pop Mhan", "40"],
    ]);
    expect(parsed.sections).toHaveLength(1);
    expect(parsed.sections[0].title).toBe("Notes");
    expect(parsed.sections[0].text).toContain(
      "J. Scott Campbell's variant cover",
    );
  });

  it("parses example 3 — 4-row table without trailing notes", () => {
    const text =
      "THE AMAZING SPIDER-MAN GETS CAUGHT UP IN CIVIL WAR II! *List of covers and their creators:* Cover | Name | Creator(s) | Sidebar Location | ------------------------------------------------------------------------------------- Reg | Regular Cover | Khary Randolph & Emilio Lopez | 1 | Var | Variant Cover | Greg Land & Morry Hollowell | 2 | Var | Variant Cover | Phil Noto | 3 | Var | Action Figure Variant Cover | John Tyler Christopher | 4 |";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    expect(parsed.tables[0].rows).toHaveLength(4);
    expect(parsed.tables[0].rows[2]).toEqual([
      "Var",
      "Variant Cover",
      "Phil Noto",
      "3",
    ]);
  });

  it("parses example 4 — 2-row table with em-dash prose", () => {
    const text =
      "To the men and women of the Marvel Universe, Ravencroft Institute for the Criminally Insane appeared to be a hospital -- as Captain America learned the hard way - some secrets have teeth. *List of covers and their creators:* Cover | Name | Creator(s) | Sidebar Location | ------------------------------------------------------------------------------- Reg | Regular Cover | Gerardo Sandoval & Romulo Fajardo Jr. | 1 | Var | Variant Cover | Greg Land & Frank D'Armata | 2 |";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    expect(parsed.intro).toContain("Ravencroft Institute");
    // The double-hyphen in the prose must not be confused with the table divider.
    expect(parsed.intro).toContain("--");
    expect(parsed.tables).toHaveLength(1);
    expect(parsed.tables[0].rows).toHaveLength(2);
  });

  it("captures the tail as a raw section when no clean divider exists", () => {
    const text =
      "Intro. *List of covers and their creators:* Cover | Name | Reg | Regular Cover |";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    expect(parsed.intro).toBe("Intro.");
    expect(parsed.tables).toHaveLength(0);
    expect(parsed.sections).toHaveLength(1);
    expect(parsed.sections[0].title).toBe("List of covers and their creators");
    expect(parsed.sections[0].text).toContain("Regular Cover");
  });

  it("reconstructs the table for smushed cover lists (Saga #1)", () => {
    // Verbatim payload pulled from the dev database — separators stripped,
    // \n\n paragraph breaks left over from the original markdown.
    const text =
      "Y: THE LAST MAN writer BRIAN K. VAUGHAN returns to comics with red-hot artist FIONA STAPLES for an all-new ONGOING SERIES! Star Wars-style action collides with Game of Thrones-esque drama in this original sci-fi/fantasy epic for mature readers, as new parents Marko and Alana risk everything to raise their child amidst a never-ending galactic war. The adventure begins in a spectacular DOUBLE-SIZED FIRST ISSUE, with forty-four pages of story with no ads for the regular price of just $2.99!\n\n*List of covers and their creators:*\nCoverNameCreatorsSidebar LocationRegRegular CoverFiona Staples1VarC2E2 Diamond Retailer Summit 2012 Exclusive Variant (Limited to 500 copies)Fiona Staples62nd Print\n\nSecond Printing CoverFiona Staples83rd PrintThird Printing CoverFiona Staples74th PrintFourth Printing CoverFiona Staples55th Print\n\nFifth Printing CoverFiona Staples4RENerd Store Exclusive Turkish EditionFiona Staples2RENerd Store Exclusive Turkish Variant EditionFiona Staples3";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    expect(parsed.intro.startsWith("Y: THE LAST MAN")).toBe(true);
    expect(parsed.intro.endsWith("just $2.99!")).toBe(true);
    expect(parsed.intro).not.toContain("CoverNameCreators");

    expect(parsed.tables).toHaveLength(1);
    const table = parsed.tables[0];
    expect(table.title).toBe("List of covers and their creators");
    expect(table.columns).toEqual(["Cover", "Name", "Creator(s)", "Sidebar"]);
    expect(table.rows).toEqual([
      ["Reg", "Regular Cover", "Fiona Staples", "1"],
      [
        "Var",
        "C2E2 Diamond Retailer Summit 2012 Exclusive Variant (Limited to 500 copies)",
        "Fiona Staples",
        "6",
      ],
      ["2nd Print", "Second Printing Cover", "Fiona Staples", "8"],
      ["3rd Print", "Third Printing Cover", "Fiona Staples", "7"],
      ["4th Print", "Fourth Printing Cover", "Fiona Staples", "5"],
      ["5th Print", "Fifth Printing Cover", "Fiona Staples", "4"],
      ["RE", "Nerd Store Exclusive Turkish Edition", "Fiona Staples", "2"],
      [
        "RE",
        "Nerd Store Exclusive Turkish Variant Edition",
        "Fiona Staples",
        "3",
      ],
    ]);
  });

  it("falls back to a 3-column smushed table when creators differ per row", () => {
    // No common creator suffix — parser should still recover rows with a
    // collapsed "Name & Creator(s)" cell so the data remains legible.
    const text =
      "Intro. *List of covers and their creators:* CoverNameCreatorsSidebar LocationRegRegular CoverArtist One1VarVariant CoverArtist Two2";
    const parsed = parseDescription(text);
    expect(parsed.hasStructuredContent).toBe(true);
    const table = parsed.tables[0];
    expect(table.columns).toEqual(["Cover", "Name & Creator(s)", "Sidebar"]);
    expect(table.rows).toEqual([
      ["Reg", "Regular CoverArtist One", "1"],
      ["Var", "Variant CoverArtist Two", "2"],
    ]);
  });
});
