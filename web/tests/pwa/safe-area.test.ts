import { describe, expect, it } from "vitest";
import {
  MIN_RESERVED_STATUS_BAR_PX,
  reservedTopInset,
  safeTopOverride,
} from "@/lib/safe-area";

// iPad Pro 11" logical size; `screen.*` never swaps with orientation on iOS.
const IPAD = { screenWidth: 834, screenHeight: 1194 };

describe("reservedTopInset", () => {
  it("is 0 when the viewport spans the whole screen (page runs under the status bar)", () => {
    expect(
      reservedTopInset({ ...IPAD, innerWidth: 834, innerHeight: 1194 }),
    ).toBe(0);
  });

  it("measures the OS-reserved status bar in portrait", () => {
    expect(
      reservedTopInset({ ...IPAD, innerWidth: 834, innerHeight: 1194 - 24 }),
    ).toBe(24);
  });

  it("uses the short screen side as full height in landscape", () => {
    expect(
      reservedTopInset({ ...IPAD, innerWidth: 1194, innerHeight: 834 - 24 }),
    ).toBe(24);
    expect(
      reservedTopInset({ ...IPAD, innerWidth: 1194, innerHeight: 834 }),
    ).toBe(0);
  });

  it("never goes negative and ignores unusable geometry", () => {
    expect(
      reservedTopInset({ ...IPAD, innerWidth: 834, innerHeight: 1300 }),
    ).toBe(0);
    expect(
      reservedTopInset({
        screenWidth: 0,
        screenHeight: 0,
        innerWidth: 800,
        innerHeight: 600,
      }),
    ).toBe(0);
  });
});

describe("safeTopOverride", () => {
  it("leaves env() alone outside standalone mode", () => {
    expect(
      safeTopOverride({ ...IPAD, innerWidth: 834, innerHeight: 1100 }, false),
    ).toBeNull();
  });

  it("pins --safe-top to 0 when the OS reserves at least a status bar", () => {
    expect(
      safeTopOverride(
        {
          ...IPAD,
          innerWidth: 834,
          innerHeight: 1194 - MIN_RESERVED_STATUS_BAR_PX,
        },
        true,
      ),
    ).toBe("0px");
    expect(
      safeTopOverride(
        { ...IPAD, innerWidth: 834, innerHeight: 1194 - 47 },
        true,
      ),
    ).toBe("0px");
  });

  it("keeps env() when the standalone viewport is edge to edge (iPadOS ≤ 26.0, iPhone)", () => {
    expect(
      safeTopOverride({ ...IPAD, innerWidth: 834, innerHeight: 1194 }, true),
    ).toBeNull();
    expect(
      safeTopOverride(
        { ...IPAD, innerWidth: 834, innerHeight: 1194 - 10 },
        true,
      ),
    ).toBeNull();
  });
});
