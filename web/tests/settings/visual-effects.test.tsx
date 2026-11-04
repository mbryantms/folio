// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const useMe = vi.fn();
vi.mock("@/lib/api/queries", () => ({ useMe: () => useMe() }));

import { EFFECTS, effectSetting, useVisualEffect } from "@/lib/visual-effects";

function setOsReduced(matches: boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: query.includes("reduced-motion") ? matches : false,
    media: query,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as unknown as typeof window.matchMedia;
}

beforeEach(() => {
  setOsReduced(false);
  useMe.mockReturnValue({ data: { effects: {}, motion: null } });
});

describe("effectSetting", () => {
  it("falls back to each effect's default when nothing is stored", () => {
    for (const e of EFFECTS) expect(effectSetting({}, e.key)).toBe(e.default);
    expect(effectSetting(undefined, "spine_stack")).toBe(true);
    expect(effectSetting(undefined, "page_curl")).toBe(false);
  });

  it("uses the stored value, including an explicit false", () => {
    expect(effectSetting({ spine_stack: false }, "spine_stack")).toBe(false);
    expect(effectSetting({ page_curl: true }, "page_curl")).toBe(true);
    expect(effectSetting({ page_curl: null }, "page_curl")).toBe(false);
  });
});

describe("useVisualEffect", () => {
  it("returns the user's switch", () => {
    useMe.mockReturnValue({
      data: { effects: { cover_tilt: true }, motion: null },
    });
    expect(renderHook(() => useVisualEffect("cover_tilt")).result.current).toBe(
      true,
    );
  });

  it("turns motion effects off under the account's reduced motion setting", () => {
    useMe.mockReturnValue({
      data: {
        effects: { cover_tilt: true, hero_backdrop: true },
        motion: "reduced",
      },
    });
    expect(renderHook(() => useVisualEffect("cover_tilt")).result.current).toBe(
      false,
    );
    // Not a motion effect, so it stays on.
    expect(
      renderHook(() => useVisualEffect("hero_backdrop")).result.current,
    ).toBe(true);
  });

  it("turns motion effects off when the OS asks for reduced motion", () => {
    setOsReduced(true);
    useMe.mockReturnValue({
      data: { effects: { page_curl: true }, motion: "system" },
    });
    expect(renderHook(() => useVisualEffect("page_curl")).result.current).toBe(
      false,
    );
  });
});
