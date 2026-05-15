import { describe, expect, it } from "vitest";

import { isAccent, isDensity, isTheme, resolvedDataTheme } from "@/lib/theme";

describe("theme guards", () => {
  it("isTheme accepts the four documented values", () => {
    expect(isTheme("dark")).toBe(true);
    expect(isTheme("light")).toBe(true);
    expect(isTheme("system")).toBe(true);
    expect(isTheme("amber")).toBe(true);
  });

  it("isTheme rejects garbage", () => {
    expect(isTheme("neon")).toBe(false);
    expect(isTheme(undefined)).toBe(false);
    expect(isTheme(null)).toBe(false);
    expect(isTheme(42)).toBe(false);
  });

  it("isAccent accepts the four palette tokens", () => {
    expect(isAccent("amber")).toBe(true);
    expect(isAccent("blue")).toBe(true);
    expect(isAccent("emerald")).toBe(true);
    expect(isAccent("rose")).toBe(true);
    expect(isAccent("teal")).toBe(false);
  });

  it("isDensity accepts the two density tokens", () => {
    expect(isDensity("comfortable")).toBe(true);
    expect(isDensity("compact")).toBe(true);
    expect(isDensity("dense")).toBe(false);
  });
});

describe("resolvedDataTheme", () => {
  // M6 (D-4) shipped curated `light` and `amber` palettes. Each
  // selectable theme now maps 1:1 to a `data-theme` attribute the CSS
  // in `globals.css` keys off. `system` stays mapped to `dark` until
  // we explicitly turn on `enableSystem` in `ThemeProvider` (a
  // separate scope item that needs FOUC-on-hydration handling).
  it("maps each curated theme to its own data-theme attribute", () => {
    expect(resolvedDataTheme("dark")).toBe("dark");
    expect(resolvedDataTheme("light")).toBe("light");
    expect(resolvedDataTheme("amber")).toBe("amber");
  });

  it("returns dark for the system / unknown / nullish cases", () => {
    expect(resolvedDataTheme("system")).toBe("dark");
    expect(resolvedDataTheme(null)).toBe("dark");
    expect(resolvedDataTheme(undefined)).toBe("dark");
  });
});
