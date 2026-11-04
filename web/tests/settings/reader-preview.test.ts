import { describe, expect, it } from "vitest";

import { previewSpreads } from "@/components/settings/ReaderPreview";

describe("previewSpreads", () => {
  it("single and webtoon views show one page at a time", () => {
    expect(previewSpreads("single", true, 3)).toEqual([[0], [1], [2]]);
    expect(previewSpreads("auto", false, 2)).toEqual([[0], [1]]);
  });
  it("double view pairs pages around a solo cover", () => {
    expect(previewSpreads("double", true, 6)).toEqual([
      [0],
      [1, 2],
      [3, 4],
      [5],
    ]);
  });
  it("double view without a solo cover pairs from the first page", () => {
    expect(previewSpreads("double", false, 5)).toEqual([[0, 1], [2, 3], [4]]);
  });
});
