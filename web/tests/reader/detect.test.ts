import { describe, expect, it } from "vitest";
import type { PageInfo } from "@/lib/api/types";
import {
  detectDirection,
  detectInitialViewMode,
  detectViewMode,
} from "@/lib/reader/detect";

const portrait = (n: number): PageInfo[] =>
  Array.from({ length: n }, (_, i) => ({
    image: i,
    image_width: 900,
    image_height: 1350,
  }));

const landscape = (n: number): PageInfo[] =>
  Array.from({ length: n }, (_, i) => ({
    image: i,
    image_width: 1800,
    image_height: 900,
  }));

const tall = (n: number): PageInfo[] =>
  Array.from({ length: n }, (_, i) => ({
    image: i,
    image_width: 800,
    image_height: 4000,
  }));

describe("detectDirection", () => {
  it("returns rtl for manga=YesAndRightToLeft regardless of user default", () => {
    expect(detectDirection("YesAndRightToLeft", "ltr")).toBe("rtl");
    expect(detectDirection("YesAndRightToLeft", null)).toBe("rtl");
    expect(detectDirection("YesAndRightToLeft", "ltr", "ltr")).toBe("rtl");
  });

  it("falls back to user default when manga is plain Yes", () => {
    expect(detectDirection("Yes", "rtl")).toBe("rtl");
    expect(detectDirection("Yes", "ltr")).toBe("ltr");
  });

  it("defaults to ltr when neither manga nor user pref is set", () => {
    expect(detectDirection(null, null)).toBe("ltr");
    expect(detectDirection(undefined, undefined)).toBe("ltr");
  });

  it("uses library default when user pref is null (M1)", () => {
    expect(detectDirection(null, null, "rtl")).toBe("rtl");
    expect(detectDirection(null, null, "ltr")).toBe("ltr");
  });

  it("user pref wins over library default when both set (M1)", () => {
    expect(detectDirection(null, "ltr", "rtl")).toBe("ltr");
    expect(detectDirection(null, "rtl", "ltr")).toBe("rtl");
  });

  it("falls back to ltr when library default is unrecognized", () => {
    // Forward-compatibility: future "ttb" or "auto" shouldn't pin
    // to an unknown value at this layer — defer to the next signal.
    expect(detectDirection(null, null, "ttb" as never)).toBe("ltr");
    expect(detectDirection(null, null, null)).toBe("ltr");
    expect(detectDirection(null, null, undefined)).toBe("ltr");
  });

  it("series override wins over user + library defaults (M2)", () => {
    // Series says RTL; user says LTR; library says LTR. Series wins.
    expect(detectDirection(null, "ltr", "ltr", "rtl")).toBe("rtl");
    expect(detectDirection(null, "rtl", "rtl", "ltr")).toBe("ltr");
  });

  it("ComicInfo Manga still wins over series override (M2)", () => {
    // Author intent is the highest layer.
    expect(detectDirection("YesAndRightToLeft", "ltr", "ltr", "ltr")).toBe(
      "rtl",
    );
  });

  it("series override skipped when null falls through to user (M2)", () => {
    // Series has no opinion → defer to user.
    expect(detectDirection(null, "rtl", "ltr", null)).toBe("rtl");
    expect(detectDirection(null, "rtl", "ltr", undefined)).toBe("rtl");
  });
});

describe("detectViewMode", () => {
  it("picks single for typical portrait pages", () => {
    expect(detectViewMode(portrait(20))).toBe("single");
  });

  it("picks webtoon for very tall pages", () => {
    expect(detectViewMode(tall(20))).toBe("webtoon");
  });

  it("picks double when most pages are landscape spreads", () => {
    expect(detectViewMode(landscape(20))).toBe("double");
  });

  it("picks double when ≥10% of pages carry the DoublePage flag", () => {
    const pages = portrait(20);
    pages[0]!.double_page = true;
    pages[1]!.double_page = true;
    expect(detectViewMode(pages)).toBe("double");
  });

  it("ignores a single DoublePage flag in a long issue (under 10%)", () => {
    const pages = portrait(20);
    pages[5]!.double_page = true;
    expect(detectViewMode(pages)).toBe("single");
  });

  it("returns single when pages array is empty", () => {
    expect(detectViewMode([])).toBe("single");
  });

  it("falls back to single when no pages have dimensions and no flag", () => {
    const pages: PageInfo[] = Array.from({ length: 10 }, (_, i) => ({
      image: i,
    }));
    expect(detectViewMode(pages)).toBe("single");
  });
});

describe("series ttb → webtoon (WP-4.2, audit UX-5)", () => {
  it("series ttb selects webtoon regardless of page shape", () => {
    expect(detectInitialViewMode(portrait(20), null, "ttb")).toBe("webtoon");
    expect(detectInitialViewMode(landscape(20), null, "ttb")).toBe("webtoon");
  });

  it("series ttb beats the user's default view mode", () => {
    expect(detectInitialViewMode(portrait(20), "double", "ttb")).toBe(
      "webtoon",
    );
  });

  it("falls back to the user default, then page detection", () => {
    expect(detectInitialViewMode(tall(10), "single", "ltr")).toBe("single");
    expect(detectInitialViewMode(tall(10), null, "rtl")).toBe("webtoon");
    expect(detectInitialViewMode(portrait(10), null, null)).toBe("single");
    expect(detectInitialViewMode(landscape(10), undefined)).toBe("double");
  });

  it("ttb is 'no opinion' for page-turn direction", () => {
    expect(detectDirection(null, "rtl", null, "ttb")).toBe("rtl");
    expect(detectDirection(null, null, "rtl", "ttb")).toBe("rtl");
    expect(detectDirection(null, null, null, "ttb")).toBe("ltr");
    expect(detectDirection("YesAndRightToLeft", null, null, "ttb")).toBe("rtl");
  });
});
