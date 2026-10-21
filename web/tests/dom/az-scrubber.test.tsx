// @vitest-environment jsdom
/**
 * `<AtoZScrubber>` — the phone letter rail. A drag previews letters and
 * commits exactly once on release; tapping the active letter clears;
 * keyboard activation on a letter button still works (pointer input is
 * handled on the rail so a drag can cross letters).
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AtoZScrubber, AZ_BUCKETS } from "@/components/library/AtoZScrubber";

function railWithHeight(height: number): HTMLElement {
  const nav = screen.getByRole("navigation", { name: "Jump to letter" });
  nav.getBoundingClientRect = () =>
    ({
      top: 0,
      height,
      left: 0,
      width: 24,
      bottom: height,
      right: 24,
    }) as DOMRect;
  nav.setPointerCapture = () => {};
  return nav;
}

describe("AtoZScrubber", () => {
  it("lists # and A–Z", () => {
    render(<AtoZScrubber value={null} onSelect={() => {}} />);
    expect(AZ_BUCKETS).toHaveLength(27);
    expect(screen.getAllByRole("button")).toHaveLength(27);
    expect(screen.getByRole("button", { name: "A" })).toBeTruthy();
  });

  it("commits once on release with the letter under the pointer", () => {
    const onSelect = vi.fn();
    render(<AtoZScrubber value={null} onSelect={onSelect} />);
    const nav = railWithHeight(270); // 10px per bucket
    fireEvent.pointerDown(nav, { clientY: 5, button: 0, pointerId: 1 });
    fireEvent.pointerMove(nav, { clientY: 15, pointerId: 1 });
    fireEvent.pointerMove(nav, { clientY: 25, pointerId: 1 });
    expect(onSelect).not.toHaveBeenCalled();
    // Letter under the finger is previewed in the bubble (B at 15..25px).
    expect(screen.getAllByText("B").length).toBeGreaterThan(1);
    fireEvent.pointerUp(nav, { clientY: 25, pointerId: 1 });
    expect(onSelect).toHaveBeenCalledOnce();
    expect(onSelect).toHaveBeenCalledWith("b");
  });

  it("clears when the active letter is tapped again", () => {
    const onSelect = vi.fn();
    render(<AtoZScrubber value="b" onSelect={onSelect} />);
    const nav = railWithHeight(270);
    fireEvent.pointerDown(nav, { clientY: 25, button: 0, pointerId: 1 });
    fireEvent.pointerUp(nav, { clientY: 25, pointerId: 1 });
    expect(onSelect).toHaveBeenCalledWith(null);
  });

  it("supports keyboard activation on a letter without double-firing", () => {
    const onSelect = vi.fn();
    render(<AtoZScrubber value={null} onSelect={onSelect} />);
    const c = screen.getByRole("button", { name: "C" });
    // Keyboard activation dispatches a click with detail 0.
    fireEvent.click(c, { detail: 0 });
    expect(onSelect).toHaveBeenCalledWith("c");
    // A mouse click (detail 1) is ignored by the button — the rail's
    // pointer handlers own it — so no second call.
    fireEvent.click(c, { detail: 1 });
    expect(onSelect).toHaveBeenCalledOnce();
  });
});
