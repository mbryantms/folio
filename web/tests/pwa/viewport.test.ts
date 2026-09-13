import { describe, expect, it } from "vitest";
import { readerViewport, themedViewport } from "@/lib/viewport";

describe("readerViewport", () => {
  it("is byte-identical to the root viewport for dark and system themes (no runtime theme-color change)", () => {
    expect(readerViewport("dark")).toEqual(themedViewport("dark"));
    expect(readerViewport("system")).toEqual(themedViewport("system"));
  });

  it("pins black/dark for light and amber themes so the reader never dresses white", () => {
    for (const theme of ["light", "amber"] as const) {
      const v = readerViewport(theme);
      expect(v.themeColor).toBe("#000000");
      expect(v.colorScheme).toBe("dark");
      expect(v).not.toEqual(themedViewport(theme));
    }
  });
});
