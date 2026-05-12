import { describe, expect, it } from "vitest";
import type { PageInfo } from "@/lib/api/types";
import { detectDirection, detectViewMode } from "@/lib/reader/detect";

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
  });

  it("falls back to user default when manga is plain Yes", () => {
    expect(detectDirection("Yes", "rtl")).toBe("rtl");
    expect(detectDirection("Yes", "ltr")).toBe("ltr");
  });

  it("defaults to ltr when neither manga nor user pref is set", () => {
    expect(detectDirection(null, null)).toBe("ltr");
    expect(detectDirection(undefined, undefined)).toBe("ltr");
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
    pages[0].double_page = true;
    pages[1].double_page = true;
    expect(detectViewMode(pages)).toBe("double");
  });

  it("ignores a single DoublePage flag in a long issue (under 10%)", () => {
    const pages = portrait(20);
    pages[5].double_page = true;
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
