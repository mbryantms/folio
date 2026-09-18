import { describe, expect, it } from "vitest";
import { readerViewport, themedViewport } from "@/lib/viewport";

describe("readerViewport", () => {
  it("is byte-identical to the root viewport for dark themes (no runtime theme-color change)", () => {
    expect(readerViewport("dark")).toEqual(themedViewport("dark"));
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

it("uses dark reader chrome even for the system light preference", () => {
  const value = readerViewport("system");
  expect(value.colorScheme).toBe("dark");
  expect(value.themeColor).toContainEqual({
    media: "(prefers-color-scheme: light)",
    color: "#000000",
  });
});
