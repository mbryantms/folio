import { describe, expect, it } from "vitest";

import {
  issueIdFromCoverUrl,
  scrubPages,
} from "@/components/library/PageScrub";
import { spineDepth } from "@/components/library/SpineStack";

describe("scrubPages", () => {
  it("starts at the cover and spreads to the last page", () => {
    expect(scrubPages(22)).toEqual([0, 4, 8, 13, 17, 21]);
  });
  it("handles short and unknown issues", () => {
    expect(scrubPages(3)).toEqual([0, 1, 2]);
    expect(scrubPages(1)).toEqual([0]);
    expect(scrubPages(Number.NaN)).toEqual([0]);
  });
});

describe("issueIdFromCoverUrl", () => {
  it("reads the issue id from a cover thumbnail URL", () => {
    expect(issueIdFromCoverUrl("/issues/abc123/pages/0/thumb")).toBe("abc123");
    expect(
      issueIdFromCoverUrl("/issues/abc/pages/0/thumb?variant=cover_small"),
    ).toBe("abc");
  });
  it("returns null for anything else", () => {
    expect(issueIdFromCoverUrl("https://cdn.example/cover.jpg")).toBeNull();
    expect(issueIdFromCoverUrl(null)).toBeNull();
  });
});

describe("spineDepth", () => {
  it("is 0 for one-shots and grows with the run, capped", () => {
    expect(spineDepth(1)).toBe(0);
    expect(spineDepth(null)).toBe(0);
    expect(spineDepth(4)).toBe(3);
    expect(spineDepth(50)).toBeGreaterThan(spineDepth(8));
    expect(spineDepth(5000)).toBe(12);
  });
});
