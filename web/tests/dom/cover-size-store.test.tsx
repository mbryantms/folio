// @vitest-environment jsdom
/**
 * The app-wide cover size store (components/library/use-cover-size.ts)
 * and its control (`CoverSizeFields`): one size for every instance, set
 * as a column count of the library row, mirrored onto <html> for the
 * grids and rails, and animated only when the reader changes it.
 */
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/ui/card-flip", () => ({
  captureCardLayout: vi.fn(),
  playCardFlip: vi.fn(),
}));

import { CoverSizeFields } from "@/components/library/CoverSizeOptions";
import {
  getCoverWidth,
  resetCoverSizeStoreForTests,
  setCoverContainerWidth,
  setCoverTarget,
  useCoverSize,
  useCoverTarget,
} from "@/components/library/use-cover-size";
import {
  COVER_SIZE_KEY,
  COVER_WIDTH_KEY,
  LEGACY_LIBRARY_SIZE_KEY,
} from "@/lib/library/cover-size";
import { captureCardLayout, playCardFlip } from "@/lib/ui/card-flip";

const v = (name: string) =>
  document.documentElement.style.getPropertyValue(name);

beforeEach(() => {
  resetCoverSizeStoreForTests();
  localStorage.clear();
  document.documentElement.removeAttribute("style");
  vi.clearAllMocks();
});

describe("useCoverSize", () => {
  it("has no column count until the library row is measured", () => {
    const { result } = renderHook(() => useCoverSize());
    expect(result.current.target).toBe(160);
    expect(result.current.columns).toBe(0);
    expect(result.current.stops).toEqual([]);
    expect(result.current.coverWidth).toBe(160);
    // Nothing to derive a target from yet.
    act(() => result.current.setColumns(8));
    expect(result.current.target).toBe(160);
    expect(localStorage.getItem(COVER_SIZE_KEY)).toBeNull();
  });

  it("derives columns, stops and the cover width from the row", () => {
    const { result } = renderHook(() => useCoverSize());
    act(() => setCoverContainerWidth(1200));
    expect(result.current.columns).toBe(6);
    expect(result.current.defaultColumns).toBe(6);
    expect(result.current.stops).toEqual([4, 5, 6, 7, 8, 9, 10]);
    expect(result.current.coverWidth).toBe(186.66);
    expect(v("--cover-min")).toBe("160px");
    expect(v("--cover-w")).toBe("186.66px");
    expect(localStorage.getItem(COVER_WIDTH_KEY)).toBe("186.66");
  });

  it("sets one size for every instance, as a column count", () => {
    const a = renderHook(() => useCoverSize());
    const b = renderHook(() => useCoverSize());
    const target = renderHook(() => useCoverTarget());
    act(() => setCoverContainerWidth(1200));
    act(() => a.result.current.setColumns(8));
    for (const h of [a, b]) {
      expect(h.result.current.target).toBe(128);
      expect(h.result.current.columns).toBe(8);
      expect(h.result.current.coverWidth).toBe(136);
    }
    expect(target.result.current).toBe(128);
    expect(localStorage.getItem(COVER_SIZE_KEY)).toBe("128");
    expect(localStorage.getItem(COVER_WIDTH_KEY)).toBe("136");
    expect(v("--cover-min")).toBe("128px");
    expect(v("--cover-w")).toBe("136px");

    act(() => b.result.current.reset());
    expect(a.result.current.target).toBe(160);
    expect(a.result.current.columns).toBe(6);
    expect(v("--cover-w")).toBe("186.66px");
  });

  it("starts from the old library slider until a size is chosen", () => {
    localStorage.setItem(LEGACY_LIBRARY_SIZE_KEY, "200");
    const { result } = renderHook(() => useCoverTarget());
    expect(result.current).toBe(200);
    act(() => setCoverTarget(140));
    expect(result.current).toBe(140);
    expect(localStorage.getItem(COVER_SIZE_KEY)).toBe("140");
  });

  it("keeps a stored target inside its bounds", () => {
    const { result } = renderHook(() => useCoverTarget());
    act(() => setCoverTarget(9999));
    expect(result.current).toBe(480);
    act(() => setCoverTarget(3));
    expect(result.current).toBe(64);
  });

  it("follows other browser tabs through the storage event", () => {
    const { result } = renderHook(() => useCoverSize());
    act(() => setCoverContainerWidth(1200));
    act(() => {
      localStorage.setItem(COVER_SIZE_KEY, "128");
      window.dispatchEvent(
        new StorageEvent("storage", { key: COVER_SIZE_KEY, newValue: "128" }),
      );
    });
    expect(result.current.target).toBe(128);
    expect(result.current.columns).toBe(8);
    expect(v("--cover-min")).toBe("128px");
    // Unrelated keys are ignored.
    act(() => {
      localStorage.setItem(COVER_SIZE_KEY, "200");
      window.dispatchEvent(
        new StorageEvent("storage", { key: "nope", newValue: "200" }),
      );
    });
    expect(result.current.target).toBe(128);
  });

  it("re-counts on a resize without changing the size", () => {
    const { result } = renderHook(() => useCoverSize());
    act(() => setCoverContainerWidth(1200));
    act(() => result.current.setColumns(8));
    act(() => setCoverContainerWidth(700));
    // 128px target on a 700px row: 4 per row (5 would need 704px),
    // each stretched to 163px.
    expect(result.current.target).toBe(128);
    expect(result.current.columns).toBe(4);
    expect(result.current.coverWidth).toBe(163);
    expect(v("--cover-w")).toBe("163px");
    expect(localStorage.getItem(COVER_SIZE_KEY)).toBe("128");
  });

  it("glides the cards for a size change, not for a resize", () => {
    const { result } = renderHook(() => useCoverSize());
    act(() => setCoverContainerWidth(1200));
    act(() => setCoverContainerWidth(900));
    expect(captureCardLayout).not.toHaveBeenCalled();
    expect(playCardFlip).not.toHaveBeenCalled();
    act(() => result.current.setColumns(7));
    expect(captureCardLayout).toHaveBeenCalledTimes(1);
    expect(playCardFlip).toHaveBeenCalledTimes(1);
    // Choosing the size already in effect is not a change.
    act(() => result.current.setColumns(7));
    expect(captureCardLayout).toHaveBeenCalledTimes(1);
  });
});

describe("getCoverWidth", () => {
  it("uses the last measured width until the row is measured again", () => {
    expect(getCoverWidth()).toBe(160);
    resetCoverSizeStoreForTests();
    localStorage.setItem(COVER_WIDTH_KEY, "186.66");
    expect(getCoverWidth()).toBe(186.66);
    act(() => setCoverContainerWidth(1000));
    // 5 per row of 1000px.
    expect(getCoverWidth()).toBe(187.2);
  });
});

describe("CoverSizeFields", () => {
  it("is a slider over covers per row", () => {
    render(<CoverSizeFields />);
    act(() => setCoverContainerWidth(1200));
    const slider = screen.getByRole("slider", { name: "Cover size" });
    expect(screen.getByText("6 per row")).toBeTruthy();
    expect(slider.getAttribute("aria-valuetext")).toBe("6 per row");
    // Compact (most per row) on the left: 10, 9, 8, 7, 6, 5, 4.
    expect(slider.getAttribute("aria-valuemin")).toBe("0");
    expect(slider.getAttribute("aria-valuemax")).toBe("6");
    expect(slider.getAttribute("aria-valuenow")).toBe("4");
    const reset = screen.getByRole("button", { name: "Reset" });
    expect(reset.hasAttribute("disabled")).toBe(true);

    // One step toward Roomy is one cover fewer per row.
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect(screen.getByText("5 per row")).toBeTruthy();
    expect(slider.getAttribute("aria-valuetext")).toBe("5 per row");
    expect(localStorage.getItem(COVER_SIZE_KEY)).toBe("207");
    expect(reset.hasAttribute("disabled")).toBe(false);

    fireEvent.click(reset);
    expect(screen.getByText("6 per row")).toBeTruthy();
  });

  it("waits for the row before offering stops", () => {
    render(<CoverSizeFields fieldId="phone-cover-size" />);
    const slider = screen.getByRole("slider", { name: "Cover size" });
    expect(slider.getAttribute("aria-valuetext")).toBeNull();
    expect(slider.getAttribute("data-disabled")).not.toBeNull();
  });
});
