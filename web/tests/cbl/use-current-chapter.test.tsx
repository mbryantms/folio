// @vitest-environment jsdom
/**
 * `useCurrentChapter`: a jump moves the reported chapter to its target at
 * once and holds it while the jump is under way; once the jump ends and
 * the page stops scrolling, the measured chapter is reported again.
 */
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useCurrentChapter } from "@/components/cbl/links/use-current-chapter";
import { pinnedControlsHeight } from "@/lib/ui/use-pinned-controls-height";

/** Three chapter sections; `tops` are their viewport offsets. */
function mountSections(tops: number[]) {
  document.body.innerHTML = "";
  tops.forEach((top, i) => {
    const el = document.createElement("section");
    el.setAttribute("data-chapter-index", String(i));
    el.getBoundingClientRect = () => ({ top }) as DOMRect;
    document.body.appendChild(el);
  });
}

describe("useCurrentChapter", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("requestAnimationFrame", (fn: FrameRequestCallback) => {
      fn(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("measures the chapter under the reading line", () => {
    mountSections([-900, -10, 400]);
    const { result } = renderHook(() => useCurrentChapter(3, null));
    expect(result.current).toBe(1);
  });

  it("holds a jump's target, then goes back to measuring", () => {
    mountSections([-900, -10, 400]);
    const { result, rerender } = renderHook(
      ({ target }: { target: number | null }) => useCurrentChapter(3, target),
      { initialProps: { target: null as number | null } },
    );
    expect(result.current).toBe(1);
    // Jump requested: the target at once, whatever the page shows.
    rerender({ target: 0 });
    expect(result.current).toBe(0);
    // The jump was handed to the browser; still held while it scrolls.
    rerender({ target: null });
    expect(result.current).toBe(0);
    // The page settles on chapter 3 (the reader scrolled on): measured.
    mountSections([-2000, -900, -10]);
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(result.current).toBe(2);
  });

  it("stays held for as long as the jump is pending", () => {
    mountSections([-900, -10, 400]);
    const { result } = renderHook(() => useCurrentChapter(3, 2));
    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(result.current).toBe(2);
  });
});

describe("pinnedControlsHeight", () => {
  function row(attrs: string, height: number, position: string) {
    const el = document.createElement("div");
    el.innerHTML = `<div ${attrs}></div>`;
    const child = el.firstElementChild as HTMLElement;
    Object.defineProperty(child, "offsetHeight", { value: height });
    child.style.position = position;
    return child;
  }

  it("is the visible sticky row's height, 0 when nothing is pinned", () => {
    const root = document.createElement("div");
    document.body.appendChild(root);
    expect(pinnedControlsHeight(root)).toBe(0);
    // The phone bar is display:none at this width (no height).
    root.appendChild(row("data-phone-controls", 0, "sticky"));
    // The desktop row wrapped onto two lines, so it is not sticky.
    const wrapped = row("data-toolbar-row data-sticky", 90, "static");
    root.appendChild(wrapped);
    expect(pinnedControlsHeight(root)).toBe(0);
    wrapped.remove();
    root.appendChild(row("data-toolbar-row data-sticky", 53, "sticky"));
    expect(pinnedControlsHeight(root)).toBe(53);
  });
});
