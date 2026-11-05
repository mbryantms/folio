import { describe, expect, it } from "vitest";

import {
  monogram,
  nameHue,
  previewTargetFor,
} from "@/components/library/PersonPreview";

describe("previewTargetFor", () => {
  it("recognises creator, character and team links", () => {
    expect(previewTargetFor("/creators/neil-gaiman")).toEqual({
      kind: "creators",
      slug: "neil-gaiman",
    });
    expect(previewTargetFor("/characters/angel")).toEqual({
      kind: "characters",
      slug: "angel",
    });
    expect(previewTargetFor("/teams/x-men")).toEqual({
      kind: "teams",
      slug: "x-men",
    });
  });
  it("ignores everything else", () => {
    expect(previewTargetFor("/arcs/secret-wars")).toBeNull();
    expect(previewTargetFor("/?library=all&credits=Neil%20Gaiman")).toBeNull();
  });
});

describe("monogram / nameHue", () => {
  it("uses the first and last initials", () => {
    expect(monogram("Neil Gaiman")).toBe("NG");
    expect(monogram("Angel")).toBe("A");
    expect(monogram("  ")).toBe("?");
  });
  it("gives a name the same hue every time", () => {
    expect(nameHue("Neil Gaiman")).toBe(nameHue("Neil Gaiman"));
    expect(nameHue("Neil Gaiman")).toBeLessThan(360);
  });
});
